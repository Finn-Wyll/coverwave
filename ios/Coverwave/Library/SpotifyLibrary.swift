import Foundation

/// The Spotify library through the Web API, with the token App Remote's
/// login returned. Mirrors the terminal app (`crates/cw-sources/src/spotify.rs`),
/// including Spotify's February 2026 Development Mode rules: playlist
/// entries come from `/playlists/{id}/items` (under `item`, or `track`
/// before), and only playlists you own or collaborate on can be listed.
@MainActor
final class SpotifyLibrary: LibraryProvider {
    let name = "spotify"
    /// Spotify's queue only grows at the end.
    let queuePositions: [QueuePosition] = [.last]

    private unowned let source: SpotifySource
    private var me: String?
    private static let base = "https://api.spotify.com/v1"
    private static let maxRows = 1000

    init(source: SpotifySource) {
        self.source = source
    }

    func reauthorize() {
        me = nil
        source.reauthorize()
    }

    // MARK: Listing

    func list(_ path: LibraryPath) async throws -> [LibraryItem] {
        switch path.id {
        case "root":
            return [
                LibraryItem(id: "liked", title: "liked songs", opens: LibraryPath(id: "liked", title: "liked songs")),
                LibraryItem(id: "playlists", title: "playlists", opens: LibraryPath(id: "playlists", title: "playlists")),
                LibraryItem(id: "albums", title: "albums", opens: LibraryPath(id: "albums", title: "albums")),
            ]
        case "liked":
            let tracks = try await all("/me/tracks").compactMap { track($0["track"]) }
            let uris = tracks.map(\.uri)
            var rows = [playAll(tracks.count) { try await self.play(uris: uris) }]
            rows += tracks.enumerated().map { i, t in
                row(t) { try await self.play(uris: Array(uris[i...])) }
            }
            return rows
        case "playlists":
            let mine = try await userID()
            return try await all("/me/playlists").compactMap { playlistRow($0, mine: mine) }
        case "albums":
            return try await all("/me/albums").compactMap { e in
                guard let a = e["album"] as? [String: Any], let id = a["id"] as? String,
                    let uri = a["uri"] as? String
                else { return nil }
                let name = a["name"] as? String ?? "Untitled"
                return LibraryItem(
                    id: "album:\(id)", title: name, subtitle: artists(a),
                    detail: (a["total_tracks"] as? Int).map(String.init),
                    opens: LibraryPath(id: "album:\(id):\(uri)", title: name))
            }
        default:
            // "playlist:ID:spotify:playlist:ID" or "album:ID:spotify:album:ID".
            let parts = path.id.split(separator: ":", maxSplits: 2).map(String.init)
            guard parts.count == 3 else { return [] }
            let (kind, id, uri) = (parts[0], parts[1], parts[2])
            let tracks: [Track]
            if kind == "playlist" {
                tracks = try await all("/playlists/\(id)/items").compactMap { e in
                    track(e["item"] is [String: Any] ? e["item"] : e["track"])
                }
            } else {
                tracks = try await all("/albums/\(id)/tracks").compactMap(track)
            }
            var rows = [playAll(tracks.count) { try await self.play(context: uri) }]
            rows += tracks.map { t in row(t) { try await self.play(context: uri, offset: t.uri) } }
            return rows
        }
    }

    func search(_ term: String) async throws -> [LibraryItem] {
        var c = URLComponents()
        c.queryItems = [
            .init(name: "q", value: term), .init(name: "type", value: "track,album,playlist"),
            .init(name: "limit", value: "10"),
        ]
        let json = try await get("/search?\(c.percentEncodedQuery ?? "")")
        func items(_ key: String) -> [[String: Any]] {
            ((json[key] as? [String: Any])?["items"] as? [Any])?.compactMap { $0 as? [String: Any] } ?? []
        }
        let playlists: [LibraryItem] = items("playlists").compactMap { p in
            guard let uri = p["uri"] as? String else { return nil }
            let name = p["name"] as? String ?? "Untitled"
            return LibraryItem(
                id: uri, title: name, subtitle: "playlist",
                play: { try await self.play(context: uri) })
        }
        let albums: [LibraryItem] = items("albums").compactMap { a in
            guard let id = a["id"] as? String, let uri = a["uri"] as? String else { return nil }
            let name = a["name"] as? String ?? "Untitled"
            return LibraryItem(
                id: uri, title: name, subtitle: artists(a),
                opens: LibraryPath(id: "album:\(id):\(uri)", title: name))
        }
        let tracks: [LibraryItem] = items("tracks").compactMap(track).map { t in
            row(t) { try await self.play(uris: [t.uri]) }
        }
        return tracks + albums + playlists
    }

    func upNext() async throws -> QueueSnapshot {
        let json = try await get("/me/player/queue")
        func entry(_ v: Any?) -> QueueSnapshot.Entry? {
            guard let t = track(v) else { return nil }
            return .init(id: UUID().uuidString, title: t.name, subtitle: t.artists)
        }
        let upcoming = (json["queue"] as? [Any] ?? []).prefix(50).compactMap(entry)
        return QueueSnapshot(current: entry(json["currently_playing"]), upcoming: Array(upcoming), note: nil)
    }

    // MARK: Rows

    private func playlistRow(_ p: [String: Any], mine: String) -> LibraryItem? {
        guard let id = p["id"] as? String, let uri = p["uri"] as? String else { return nil }
        let name = p["name"] as? String ?? "Untitled"
        let ownerInfo = p["owner"] as? [String: Any] ?? [:]
        let owner = ownerInfo["id"] as? String ?? ""
        let ownerName = ownerInfo["display_name"] as? String ?? owner
        let readable = owner == mine || (p["collaborative"] as? Bool ?? false)
        let items = p["items"] as? [String: Any]
        let tracks = p["tracks"] as? [String: Any]
        let total: Int? = (items?["total"] as? Int) ?? (tracks?["total"] as? Int)
        var row = LibraryItem(
            id: "playlist:\(id)", title: name,
            subtitle: owner == mine ? nil : "by \(ownerName)",
            detail: total.map { String($0) })
        if readable {
            row.opens = LibraryPath(id: "playlist:\(id):\(uri)", title: name)
        } else {
            // Others' playlists can be played but not listed.
            row.play = { [weak self] in try await self?.play(context: uri) }
        }
        return row
    }

    private struct Track {
        var uri: String
        var name: String
        var artists: String?
        var durationMS: Int?
    }

    private func track(_ v: Any?) -> Track? {
        guard let t = v as? [String: Any], !(t["is_local"] as? Bool ?? false),
            let uri = t["uri"] as? String
        else { return nil }
        let show = (t["show"] as? [String: Any])?["name"] as? String
        return Track(
            uri: uri, name: t["name"] as? String ?? "Untitled",
            artists: artists(t) ?? show, durationMS: t["duration_ms"] as? Int)
    }

    private func artists(_ v: [String: Any]) -> String? {
        let names = (v["artists"] as? [Any])?.compactMap { ($0 as? [String: Any])?["name"] as? String } ?? []
        return names.isEmpty ? nil : names.joined(separator: ", ")
    }

    private func row(_ t: Track, play: @escaping @MainActor () async throws -> Void) -> LibraryItem {
        LibraryItem(
            id: UUID().uuidString, title: t.name, subtitle: t.artists,
            detail: t.durationMS.map { timeText(TimeInterval($0) / 1000) },
            play: play,
            enqueue: { [weak self] _ in try await self?.source.enqueue(t.uri) })
    }

    private func playAll(_ n: Int, _ play: @escaping @MainActor () async throws -> Void) -> LibraryItem {
        LibraryItem(id: "play-all", title: "play all", detail: "\(n)", play: play)
    }

    // MARK: Playback

    private func play(context: String, offset: String? = nil) async throws {
        var body: [String: Any] = ["context_uri": context]
        if let offset { body["offset"] = ["uri": offset] }
        try await startPlayback(body, fallback: offset ?? context)
    }

    private func play(uris: [String]) async throws {
        guard let first = uris.first else { return }
        try await startPlayback(["uris": Array(uris.prefix(100))], fallback: first)
    }

    /// `PUT /me/player/play`; with no active device, App Remote plays on
    /// this iPhone instead (without the position in the list).
    private func startPlayback(_ body: [String: Any], fallback: String) async throws {
        let (status, _) = try await send("PUT", "/me/player/play", body: body)
        switch status {
        case 200..<300: return
        case 404: try await source.play(uri: fallback)
        case 403: throw LibraryError.message("Spotify Premium is needed to start playback from here.")
        default: throw LibraryError.message("Spotify said \(status).")
        }
    }

    // MARK: HTTP

    private func userID() async throws -> String {
        if let me { return me }
        let id = try await get("/me")["id"] as? String ?? ""
        me = id
        return id
    }

    /// Every page of a paged listing, up to `maxRows`.
    private func all(_ path: String) async throws -> [[String: Any]] {
        var out: [[String: Any]] = []
        var offset = 0
        while out.count < Self.maxRows {
            let sep = path.contains("?") ? "&" : "?"
            let page = try await get("\(path)\(sep)limit=50&offset=\(offset)")
            let items = (page["items"] as? [Any] ?? []).compactMap { $0 as? [String: Any] }
            out += items
            offset += 50
            if items.isEmpty || page["next"] is NSNull || page["next"] == nil { break }
        }
        return out
    }

    private func get(_ path: String) async throws -> [String: Any] {
        let (status, data) = try await send("GET", path, body: nil)
        guard (200..<300).contains(status) else {
            throw LibraryError.message("Spotify said \(status).")
        }
        if data.isEmpty { return [:] }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
    }

    private func send(_ method: String, _ path: String, body: [String: Any]?) async throws -> (Int, Data) {
        guard let token = source.webToken else { throw LibraryError.needsLogin }
        guard let url = URL(string: Self.base + path) else { throw LibraryError.message("Bad request.") }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if let body {
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let (data, response) = try await URLSession.shared.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        // 401: expired. 403 on reads: the login lacks the library scopes.
        if status == 401 || (status == 403 && method == "GET") { throw LibraryError.needsLogin }
        return (status, data)
    }
}
