import CryptoKit
import Foundation

/// Where a song's lyrics stand.
enum LyricsStatus: Equatable {
    case searching
    case found
    case instrumental
    case notFound
}

/// Lyrics from LRCLIB (lrclib.net), a free, keyless database of time-synced
/// lyrics, like the terminal app (`crates/cw-sources/src/lrclib.rs`). Sends
/// the song's artist, title, album and length. Kept in Application Support
/// (offline/lyrics), so a song is looked up once (misses again after a week)
/// and its lyrics are there offline.
enum LyricsLookup {
    private struct Record: Decodable {
        let instrumental: Bool?
        let syncedLyrics: String?
        let plainLyrics: String?
        let duration: Double?
    }

    private static let userAgent = "coverwave-ios/0.1"
    private static let instrumentalMark = "[instrumental]"
    private static let missTTL: TimeInterval = 7 * 24 * 3600

    /// Without `network`, only what was saved.
    static func lyrics(for np: NowPlaying, network: Bool = true) async -> (LyricSheet?, LyricsStatus) {
        let file = cacheFile(np.trackID)
        if let text = try? String(contentsOf: file, encoding: .utf8) {
            let age = (try? file.resourceValues(forKeys: [.contentModificationDateKey]))?
                .contentModificationDate.map { Date().timeIntervalSince($0) } ?? 0
            if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                if age < missTTL { return (nil, .notFound) }
            } else if text == instrumentalMark {
                return (nil, .instrumental)
            } else if let sheet = parseLyrics(text: text) {
                return (sheet, .found)
            }
        }
        guard network, let found = await fetch(np) else { return (nil, .notFound) }
        try? FileManager.default.createDirectory(
            at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        switch found {
        case .some(.none):
            try? instrumentalMark.write(to: file, atomically: true, encoding: .utf8)
            return (nil, .instrumental)
        case .some(.some(let text)):
            try? text.write(to: file, atomically: true, encoding: .utf8)
            if let sheet = parseLyrics(text: text) { return (sheet, .found) }
            return (nil, .notFound)
        case .none:
            try? "".write(to: file, atomically: true, encoding: .utf8)
            return (nil, .notFound)
        }
    }

    /// Whether this song's answer is saved (found, instrumental or a miss).
    static func isSaved(_ trackID: String) -> Bool {
        FileManager.default.fileExists(atPath: cacheFile(trackID).path(percentEncoded: false))
    }

    /// `nil`: LRCLIB could not be reached. `.some(nil)`: not found.
    /// `.some(.some(nil))`: instrumental. Else the LRC or plain text.
    private static func fetch(_ np: NowPlaying) async -> String??? {
        let artist = (np.artist.split(separator: ",").first.map(String.init) ?? np.artist)
            .trimmingCharacters(in: .whitespaces)
        guard !artist.isEmpty, !np.title.isEmpty else { return .some(nil) }
        var query = [
            URLQueryItem(name: "artist_name", value: artist),
            URLQueryItem(name: "track_name", value: np.title),
        ]
        if !np.album.isEmpty { query.append(URLQueryItem(name: "album_name", value: np.album)) }
        if let d = np.duration { query.append(URLQueryItem(name: "duration", value: String(Int(d.rounded())))) }
        guard let first = await get("https://lrclib.net/api/get", query) else { return nil }
        if let data = first, let record = try? JSONDecoder().decode(Record.self, from: data),
            let found = pick(record)
        {
            return .some(found)
        }
        guard let second = await get("https://lrclib.net/api/search", Array(query.prefix(2))) else { return nil }
        guard let data = second, let records = try? JSONDecoder().decode([Record].self, from: data)
        else { return .some(nil) }
        // The closest synced version within 10s of the song's length.
        let off = { (r: Record) -> Double in
            guard let a = r.duration, let b = np.duration else { return 0 }
            return abs(a - b)
        }
        let close = records.filter { off($0) <= 10 }
        let synced = close.filter { !($0.syncedLyrics ?? "").trimmingCharacters(in: .whitespaces).isEmpty }
            .min { off($0) < off($1) }
        return .some((synced ?? close.first { pick($0) != nil }).flatMap(pick))
    }

    private static func pick(_ r: Record) -> String?? {
        if r.instrumental == true { return .some(nil) }
        for text in [r.syncedLyrics, r.plainLyrics] {
            if let t = text, !t.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return .some(t) }
        }
        return nil
    }

    /// `nil`: no answer (offline). `.some(nil)`: an answer that is not 200.
    private static func get(_ base: String, _ query: [URLQueryItem]) async -> Data?? {
        guard var c = URLComponents(string: base) else { return .some(nil) }
        c.queryItems = query
        guard let url = c.url else { return .some(nil) }
        var request = URLRequest(url: url)
        request.setValue(userAgent, forHTTPHeaderField: "User-Agent")
        guard let (data, response) = try? await URLSession.shared.data(for: request) else { return nil }
        return (response as? HTTPURLResponse)?.statusCode == 200 ? .some(data) : .some(nil)
    }

    private static func cacheFile(_ trackID: String) -> URL {
        let name = SHA256.hash(data: Data(trackID.utf8)).prefix(12).map { String(format: "%02x", $0) }.joined()
        // Kept with what is saved for offline (Caches can be cleared by iOS).
        return OfflineStore.folder.appending(path: "lyrics/\(name).lrc")
    }
}
