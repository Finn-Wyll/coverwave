import SwiftUI

/// Ties it together: follows the active source, analyses each new cover in
/// the Rust core, looks up the tempo, and supplies the audio levels that
/// drive the motion.
@MainActor
@Observable
final class AppModel {
    let settings = AppSettings()
    let core = CoverCore()
    let spotify = SpotifySource()
    let appleMusic = AppleMusicSource()

    private(set) var activeKind: SourceKind?
    /// The analysed cover of the current track (nil: idle scene).
    private(set) var cover: AnalysedCover?
    /// Identity of the shown cover (and genre), for the cross-fade.
    private(set) var coverHash: String?
    /// The one accent colour, from the cover's palette.
    private(set) var accent: Color = Theme.defaultAccent
    /// Where the motion's rhythm comes from, for Settings.
    private(set) var tempoText = "—"

    @ObservationIgnored private var coverTask: Task<Void, Never>?
    /// The track and genre the shown cover was built for.
    @ObservationIgnored private var coverFor: CoverKey?

    private struct CoverKey: Equatable {
        var trackID: String
        var genre: String?
        var artworkURL: URL?
    }
    /// Rhythm map per track ID (nil: looked up, nothing found).
    @ObservationIgnored private var rhythms: [String: Rhythm?] = [:]
    @ObservationIgnored private var pending: Set<String> = []


    var active: (any MusicSource)? {
        switch activeKind {
        case .spotify: spotify
        case .appleMusic: appleMusic
        case nil: nil
        }
    }

    var nowPlaying: NowPlaying? { active?.nowPlaying }

    @ObservationIgnored private var browsers: [SourceKind: LibraryBrowser] = [:]

    /// Library browsing for the active source (kept per source, so its
    /// place and cache survive closing the panel).
    var library: LibraryBrowser? {
        guard let kind = activeKind else { return nil }
        if let b = browsers[kind] { return b }
        let provider: any LibraryProvider =
            switch kind {
            case .spotify: SpotifyLibrary(source: spotify)
            case .appleMusic: AppleMusicLibrary()
            }
        let b = LibraryBrowser(provider: provider)
        browsers[kind] = b
        return b
    }

    /// A source is connected (the player panel shows instead of the
    /// source chooser).
    var isFollowing: Bool { active?.status == .connected }

    init() {
        spotify.onChange = { [weak self] np in self?.trackChanged(np, from: .spotify) }
        appleMusic.onChange = { [weak self] np in self?.trackChanged(np, from: .appleMusic) }
    }

    func start() async {
        if let last = settings.lastSource {
            activeKind = last
            source(last).resume()
        }
        await core.loadIfPresent()
        if case .missing = core.modelState { await core.download() }
        modelBecameReady()
    }

    /// Downloads the cover-analysis model (from Settings, after a failure).
    func downloadModel() async {
        await core.download()
        modelBecameReady()
    }

    func source(_ kind: SourceKind) -> any MusicSource {
        switch kind {
        case .spotify: spotify
        case .appleMusic: appleMusic
        }
    }

    /// Makes `kind` the active source (from a user action: it may ask for
    /// permission).
    func choose(_ kind: SourceKind) {
        if let current = activeKind, current != kind { source(current).disconnect() }
        activeKind = kind
        settings.lastSource = kind
        source(kind).connect()
        trackChanged(source(kind).nowPlaying, from: kind)
    }

    func handleOpenURL(_ url: URL) {
        _ = spotify.handle(url: url)
    }

    func sceneBecameActive() {
        active?.resume()
    }

    // MARK: Covers

    private func trackChanged(_ np: NowPlaying?, from kind: SourceKind) {
        guard kind == activeKind else { return }
        guard let np else {
            coverTask?.cancel()
            coverFor = nil
            setCover(nil)
            return
        }
        lookUpRhythm(np)
        // A new track, or its genre or cover URL arrived (sources report
        // them a moment later). A cover whose embedding is cached rebuilds
        // instantly.
        let key = CoverKey(trackID: np.trackID, genre: np.genre, artworkURL: np.artworkURL)
        if key == coverFor { return }
        coverFor = key
        coverTask?.cancel()
        coverTask = Task { [weak self] in
            guard let self, let source = self.active else { return }
            guard let image = await source.artwork(for: np) else {
                if !Task.isCancelled { self.setCover(nil) }
                return
            }
            // Superseded while the image loaded (e.g. the genre arrived).
            guard !Task.isCancelled else { return }
            let cover = try? await self.core.analyse(image: image, genre: np.genre)
            guard !Task.isCancelled, self.nowPlaying?.trackID == np.trackID else { return }
            self.setCover(cover)
        }
    }

    private func setCover(_ cover: AnalysedCover?) {
        self.cover = cover
        coverHash = cover.map { "\($0.hash())-\($0.genreFamily())" }
        let c = cover?.accent()
        accent = c.map { Color(red: Double($0.r) / 255, green: Double($0.g) / 255, blue: Double($0.b) / 255) }
            ?? Theme.defaultAccent
        refreshTempoText()
    }

    /// Re-analyses the current cover once the model has loaded, so covers
    /// shown before it get their real scenes.
    private func modelBecameReady() {
        guard core.modelState == .ready, let kind = activeKind, let np = nowPlaying,
            cover?.isComplete() == false
        else { return }
        coverFor = nil
        trackChanged(np, from: kind)
    }

    // MARK: Motion

    private func lookUpRhythm(_ np: NowPlaying) {
        guard settings.tempoLookup, rhythms[np.trackID] == nil else { return refreshTempoText() }
        rhythms[np.trackID] = .some(nil)
        pending.insert(np.trackID)
        refreshTempoText()
        Task { [weak self] in
            let rhythm = await RhythmLookup.rhythm(for: np)
            self?.rhythms[np.trackID] = .some(rhythm)
            self?.pending.remove(np.trackID)
            self?.refreshTempoText()
        }
    }

    private func rhythm(_ trackID: String) -> Rhythm? {
        settings.tempoLookup ? rhythms[trackID] ?? nil : nil
    }

    private func refreshTempoText() {
        guard let np = nowPlaying else {
            tempoText = "—"
            return
        }
        if let r = rhythm(np.trackID) {
            tempoText =
                "\(Int(r.bpm().rounded())) BPM in \(r.beatsPerBar())/4, measured from the song"
                + (r.isAligned() ? "" : " (bar start estimated)")
        } else if settings.tempoLookup, pending.contains(np.trackID) {
            tempoText = "Measuring the song's rhythm…"
        } else {
            let family = np.genre.map(genreFamily) ?? "unknown"
            let typical = cover?.typicalBpm() ?? 110
            tempoText = "\(Int(typical.rounded())) BPM, typical for \(Self.familyName(family))"
        }
    }

    static func familyName(_ family: String) -> String {
        switch family {
        case "unknown": "an unknown genre"
        case "hip_hop": "hip-hop"
        case "rnb_soul": "R&B and soul"
        default: family.replacingOccurrences(of: "_", with: " ")
        }
    }

    private static let silence = AudioLevels(
        bands: Array(repeating: 0, count: 6), level: 0, onset: 0, onsetCount: 0)

    /// What drives the motion at `date`: the song's rhythm map against the
    /// playback position; else a beat clock at the genre's typical tempo.
    func levels(at date: Date) -> AudioLevels {
        guard let np = nowPlaying else { return Self.silence }
        let position = np.position(at: date)
        guard let r = rhythm(np.trackID) else {
            return beatLevels(bpm: 0, genre: np.genre, positionSecs: position, playing: np.isPlaying)
        }
        return r.levels(positionSecs: position, playing: np.isPlaying, downbeatSecs: r.defaultDownbeat())
    }
}
