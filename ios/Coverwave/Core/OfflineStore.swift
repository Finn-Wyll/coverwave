import CryptoKit
import Foundation

/// What coverwave keeps for listening without the internet, like the
/// terminal app (`crates/cw-sources/src/offline.rs`): per Spotify track its
/// details and cover, and the library listings last seen or saved ahead.
/// Lyrics and rhythm maps are kept next to it by their own lookups.
///
/// Spotify's downloaded songs stay inside the Spotify app (encrypted, and
/// iOS keeps each app's files private); offline, coverwave follows that app
/// and fills in the rest from here. Everything lives in Application Support,
/// which iOS does not clear on its own, unlike Caches.
@MainActor
@Observable
final class OfflineStore {
    static let shared = OfflineStore()

    nonisolated static var folder: URL { URL.applicationSupportDirectory.appending(path: "offline") }

    struct Record: Codable, Equatable {
        /// Spotify URI (as `NowPlaying.trackID`).
        var id: String
        var title: String
        var artist: String
        var album: String
        var duration: Double?
        var isrc: String?
        var artworkURL: String?
        var genre: String?
        var genreChecked = false
        var lyricsChecked = false
        var rhythmChecked = false
        var coverChecked = false

        var nowPlaying: NowPlaying {
            NowPlaying(
                trackID: id, title: title, artist: artist, album: album, duration: duration,
                position: 0, readAt: .distantPast, isPlaying: false, genre: genre, isrc: isrc,
                artworkURL: artworkURL.flatMap(URL.init(string:)))
        }
    }

    /// A listing row without its actions, so it can be saved.
    struct Row: Codable, Equatable {
        enum Action: Codable, Equatable {
            /// Play a context (album or playlist), from one of its tracks.
            case context(uri: String, offset: String?, index: Int?)
            /// Play these tracks in order.
            case tracks([String])
        }

        var id: String
        var title: String
        var subtitle: String?
        var detail: String?
        var opensID: String?
        var opensTitle: String?
        var action: Action?
        /// The track to queue, if it can be queued.
        var trackURI: String?
    }

    private(set) var records: [String: Record] = [:]
    /// Bumped on every change, so views showing counts refresh.
    private(set) var revision = 0

    @ObservationIgnored private var saveTask: Task<Void, Never>?

    private init() {
        if let data = try? Data(contentsOf: Self.folder.appending(path: "tracks.json")),
            let saved = try? JSONDecoder().decode([String: Record].self, from: data)
        {
            records = saved
        }
    }

    // MARK: Tracks

    func record(_ id: String) -> Record? { records[id] }

    /// Adds or updates a track, keeping what is already known.
    func remember(_ r: Record) {
        var merged = records[r.id] ?? r
        if records[r.id] != nil {
            merged.title = r.title.isEmpty ? merged.title : r.title
            merged.artist = r.artist.isEmpty ? merged.artist : r.artist
            merged.album = r.album.isEmpty ? merged.album : r.album
            merged.duration = r.duration ?? merged.duration
            merged.isrc = r.isrc ?? merged.isrc
            merged.artworkURL = r.artworkURL ?? merged.artworkURL
            merged.genre = r.genre ?? merged.genre
            merged.genreChecked = merged.genreChecked || r.genreChecked
            merged.lyricsChecked = merged.lyricsChecked || r.lyricsChecked
            merged.rhythmChecked = merged.rhythmChecked || r.rhythmChecked
            merged.coverChecked = merged.coverChecked || r.coverChecked
        }
        guard records[r.id] != merged else { return }
        records[r.id] = merged
        changed()
    }

    /// Remembers what a source reports for the playing track.
    func remember(_ np: NowPlaying) {
        guard np.trackID.hasPrefix("spotify:") else { return }
        remember(Record(
            id: np.trackID, title: np.title, artist: np.artist, album: np.album,
            duration: np.duration, isrc: np.isrc, artworkURL: np.artworkURL?.absoluteString,
            genre: np.genre, genreChecked: np.genre != nil))
    }

    func update(_ id: String, _ change: (inout Record) -> Void) {
        guard var r = records[id] else { return }
        change(&r)
        guard r != records[id] else { return }
        records[id] = r
        changed()
    }

    /// Saved tracks matching every word of `term`.
    func search(_ term: String, limit: Int = 50) -> [Record] {
        let words = term.lowercased().split(separator: " ").map(String.init)
        guard !words.isEmpty else { return [] }
        return records.values
            .filter { r in
                let hay = "\(r.title) \(r.artist) \(r.album)".lowercased()
                return words.allSatisfy { hay.contains($0) }
            }
            .sorted { $0.title.lowercased() < $1.title.lowercased() }
            .prefix(limit)
            .map { $0 }
    }

    /// Fills in what a source left out (genre, ISRC, cover URL).
    func enrich(_ np: NowPlaying) -> NowPlaying {
        guard let r = records[np.trackID] else { return np }
        var out = np
        out.genre = out.genre ?? r.genre
        out.isrc = out.isrc ?? r.isrc
        out.artworkURL = out.artworkURL ?? r.artworkURL.flatMap(URL.init(string:))
        return out
    }

    // MARK: Covers

    /// The saved cover: the one downloaded from its URL (the same bytes as
    /// online, so the same motion), else one the Spotify app gave.
    func cover(for np: NowPlaying) -> Data? {
        let url = np.artworkURL?.absoluteString ?? records[np.trackID]?.artworkURL
        if let url, let data = try? Data(contentsOf: coverFile(url)) { return data }
        return try? Data(contentsOf: coverFile(np.trackID))
    }

    func hasCover(url: String) -> Bool {
        FileManager.default.fileExists(atPath: coverFile(url).path(percentEncoded: false))
    }

    /// Saves a cover under its URL, or under the track when there is none.
    func saveCover(_ data: Data, key: String) {
        write(data, to: coverFile(key))
    }

    private func coverFile(_ key: String) -> URL {
        Self.folder.appending(path: "covers/\(Self.hashed(key)).img")
    }

    // MARK: Listings

    func saveListing(_ pathID: String, _ rows: [Row]) {
        guard let data = try? JSONEncoder().encode(rows) else { return }
        write(data, to: listingFile(pathID))
    }

    func listing(_ pathID: String) -> [Row]? {
        guard let data = try? Data(contentsOf: listingFile(pathID)) else { return nil }
        return try? JSONDecoder().decode([Row].self, from: data)
    }

    private func listingFile(_ pathID: String) -> URL {
        Self.folder.appending(path: "library/\(Self.hashed(pathID)).json")
    }

    // MARK: Size

    /// Bytes on disk: this store plus saved lyrics and rhythm maps.
    nonisolated static func diskBytes() -> Int64 {
        let fm = FileManager.default
        guard let walker = fm.enumerator(at: folder, includingPropertiesForKeys: [.fileSizeKey]) else { return 0 }
        var total: Int64 = 0
        for case let url as URL in walker {
            total += Int64((try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
        }
        return total
    }

    /// Forgets everything saved for offline.
    func clear() {
        saveTask?.cancel()
        try? FileManager.default.removeItem(at: Self.folder)
        records = [:]
        revision += 1
    }

    // MARK: Files

    nonisolated static func hashed(_ key: String) -> String {
        SHA256.hash(data: Data(key.utf8)).prefix(12).map { String(format: "%02x", $0) }.joined()
    }

    private func write(_ data: Data, to url: URL) {
        try? FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? data.write(to: url, options: .atomic)
    }

    /// Writes the track index a moment after the last change.
    private func changed() {
        revision += 1
        saveTask?.cancel()
        saveTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard let self, !Task.isCancelled,
                let data = try? JSONEncoder().encode(self.records)
            else { return }
            self.write(data, to: Self.folder.appending(path: "tracks.json"))
        }
    }
}
