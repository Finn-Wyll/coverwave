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
    private(set) var micActive = false

    @ObservationIgnored private let mic = MicListener()
    @ObservationIgnored private var coverTask: Task<Void, Never>?
    /// The track and genre the shown cover was built for.
    @ObservationIgnored private var coverFor: CoverKey?

    private struct CoverKey: Equatable {
        var trackID: String
        var genre: String?
        var artworkURL: URL?
    }
    /// Tempo per track ID (nil: looked up, nothing found).
    @ObservationIgnored private var tempos: [String: Float?] = [:]

    var active: (any MusicSource)? {
        switch activeKind {
        case .spotify: spotify
        case .appleMusic: appleMusic
        case nil: nil
        }
    }

    var nowPlaying: NowPlaying? { active?.nowPlaying }

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
        if settings.listenWithMicrophone { await setMicrophone(true) }
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
        lookUpTempo(np)
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

    private func lookUpTempo(_ np: NowPlaying) {
        guard settings.tempoLookup, tempos[np.trackID] == nil else { return refreshTempoText() }
        tempos[np.trackID] = .some(nil)
        Task { [weak self] in
            let bpm = await TempoLookup.bpm(for: np)
            self?.tempos[np.trackID] = .some(bpm)
            self?.refreshTempoText()
        }
    }

    private func knownTempo(_ trackID: String) -> Float? {
        settings.tempoLookup ? tempos[trackID] ?? nil : nil
    }

    private func refreshTempoText() {
        guard let np = nowPlaying else {
            tempoText = "—"
            return
        }
        if micActive {
            tempoText = "Microphone"
        } else if let bpm = knownTempo(np.trackID) {
            tempoText = "\(Int(bpm.rounded())) BPM"
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

    func setMicrophone(_ on: Bool) async {
        settings.listenWithMicrophone = on
        if on {
            micActive = await mic.start()
            if !micActive { settings.listenWithMicrophone = false }
        } else {
            mic.stop()
            micActive = false
        }
        refreshTempoText()
    }

    private static let silence = AudioLevels(
        bands: Array(repeating: 0, count: 6), level: 0, onset: 0, onsetCount: 0)

    /// What drives the motion at `date`: the microphone when it hears the
    /// music, else a beat clock at the track's tempo with its genre's rhythm.
    func levels(at date: Date) -> AudioLevels {
        guard let np = nowPlaying else { return Self.silence }
        if micActive, np.isPlaying, let heard = mic.levels(at: date) { return heard }
        return beatLevels(
            bpm: knownTempo(np.trackID) ?? 0,
            genre: np.genre,
            positionSecs: np.position(at: date),
            playing: np.isPlaying)
    }
}
