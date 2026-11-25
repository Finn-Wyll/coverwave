import Foundation

/// Saves ahead, while online, what Spotify songs need to play offline with
/// their own motion, like the terminal app (`frontends/cw-tui/src/offline_saver.rs`):
/// the liked songs' (and playlists') listings, then per song its cover (and
/// the cover's analysis), genre, lyrics and rhythm map.
///
/// The songs themselves are downloaded in the Spotify app; coverwave cannot
/// see which ones are. Runs while coverwave is open, one song at a time,
/// only on Wi-Fi unless mobile data is allowed, and stops at the first sign
/// of being offline. The library is listed again every 12 hours.
@MainActor
@Observable
final class OfflineSaver {
    enum Scope: String, CaseIterable, Identifiable {
        case off
        case liked
        case all

        var id: String { rawValue }

        var label: String {
            switch self {
            case .off: "Off"
            case .liked: "Liked Songs"
            case .all: "Liked + Playlists"
            }
        }
    }

    /// What it is doing, for Settings.
    private(set) var activity: String?
    /// Songs with everything saved, of those known.
    private(set) var ready = 0
    private(set) var known = 0
    private(set) var diskBytes: Int64 = 0

    @ObservationIgnored private var task: Task<Void, Never>?
    @ObservationIgnored private unowned let model: AppModel
    private static let relistEvery: TimeInterval = 12 * 3600
    private static let lastListKey = "offline.lastList"

    init(model: AppModel) {
        self.model = model
    }

    private var settings: AppSettings { model.settings }
    private var store: OfflineStore { .shared }

    /// Whether the network may be used for saving now.
    private var allowed: Bool {
        let net = NetworkMonitor.shared
        return settings.saveAhead != .off && model.activeKind != .offline && net.isOnline
            && (settings.saveOnMobileData || !net.isExpensive)
    }

    /// Starts a pass if one is due and allowed (cheap to call often).
    func kick(force: Bool = false) {
        refreshCounts()
        guard task == nil, allowed, model.spotify.isConfigured, model.spotify.status == .connected,
            !model.spotify.followingApp
        else { return }
        task = Task { [weak self] in
            await self?.pass(force: force)
            self?.task = nil
            self?.activity = nil
            self?.refreshCounts()
        }
    }

    func stop() {
        task?.cancel()
        task = nil
        activity = nil
    }

    /// Forgets everything saved for offline.
    func deleteAll() {
        stop()
        store.clear()
        UserDefaults.standard.removeObject(forKey: Self.lastListKey)
        refreshCounts()
    }

    func refreshCounts() {
        let records = Array(store.records.values)
        known = records.count
        ready = records.filter(isComplete).count
        Task.detached(priority: .utility) {
            let bytes = OfflineStore.diskBytes()
            await MainActor.run { [weak self] in self?.diskBytes = bytes }
        }
    }

    private func isComplete(_ r: OfflineStore.Record) -> Bool {
        r.coverChecked && r.genreChecked && r.lyricsChecked && (r.rhythmChecked || !settings.tempoLookup)
    }

    // MARK: A pass

    private func pass(force: Bool) async {
        let last = UserDefaults.standard.double(forKey: Self.lastListKey)
        if force || Date().timeIntervalSince1970 - last > Self.relistEvery {
            guard await listLibrary() else { return }
            UserDefaults.standard.set(Date().timeIntervalSince1970, forKey: Self.lastListKey)
        }
        let todo = store.records.values.filter { !isComplete($0) }.sorted { $0.title < $1.title }
        for (i, r) in todo.enumerated() {
            guard !Task.isCancelled, allowed else { return }
            activity = "Saving \(i + 1) of \(todo.count): \(r.title)"
            guard await save(r) else { return }
            if i % 10 == 0 { refreshCounts() }
        }
    }

    /// Lists (and so saves) liked songs and, with `.all`, playlists.
    private func listLibrary() async -> Bool {
        guard let library = model.spotifyLibrary else { return false }
        do {
            activity = "Listing liked songs"
            _ = try await library.rows(LibraryPath(id: "liked", title: "liked songs"))
            guard settings.saveAhead == .all else { return true }
            activity = "Listing playlists"
            let playlists = try await library.rows(LibraryPath(id: "playlists", title: "playlists"))
            for p in playlists {
                guard !Task.isCancelled, allowed else { return false }
                guard let id = p.opensID else { continue }
                activity = "Listing \(p.title)"
                _ = try? await library.rows(LibraryPath(id: id, title: p.opensTitle ?? p.title))
            }
            return true
        } catch {
            activity = nil
            return false
        }
    }

    /// Fills in one song. False when the internet went away.
    private func save(_ r: OfflineStore.Record) async -> Bool {
        let np = r.nowPlaying
        if !r.coverChecked {
            if let url = np.artworkURL, !store.hasCover(url: url.absoluteString) {
                guard let (data, response) = try? await URLSession.shared.data(from: url) else { return false }
                if (response as? HTTPURLResponse)?.statusCode == 200 {
                    store.saveCover(data, key: url.absoluteString)
                    // The cover's analysis is cached too (when the model is ready).
                    if model.core.modelState == .ready {
                        _ = try? await model.core.analyse(image: data, genre: r.genre)
                    }
                }
            }
            store.update(r.id) { $0.coverChecked = true }
        }
        if !r.genreChecked {
            let genre = await GenreLookup.genre(for: np)
            store.update(r.id) {
                $0.genre = $0.genre ?? genre
                $0.genreChecked = true
            }
        }
        if !r.lyricsChecked {
            _ = await LyricsLookup.lyrics(for: np)
            // Saved only when LRCLIB answered.
            guard LyricsLookup.isSaved(r.id) else { return false }
            store.update(r.id) { $0.lyricsChecked = true }
        }
        if settings.tempoLookup, !r.rhythmChecked {
            _ = await RhythmLookup.rhythm(for: np)
            guard NetworkMonitor.shared.isOnline else { return false }
            store.update(r.id) { $0.rhythmChecked = true }
        }
        return true
    }
}
