import Foundation

/// The Spotify library through the Web API, with `SpotifySource`'s login. Mirrors the terminal app (`crates/cw-sources/src/spotify.rs`),
/// including Spotify's February 2026 Development Mode rules: playlist
/// entries come from `/playlists/{id}/items` (under `item`, or `track`
/// before), and only playlists you own or collaborate on can be listed.
///
/// Every listing is saved (`OfflineStore`), with its tracks' details, so the
/// library still opens offline; rows then play through the Spotify app.
@MainActor
final class SpotifyLibrary: LibraryProvider {
    let name = "spotify"
    /// Spotify's queue only grows at the end.
    let queuePositions: [QueuePosition] = [.last]

    private unowned let source: SpotifySource
    private var me: String?
    private static let maxRows = 1000

    init(source: SpotifySource) {
        self.source = source
    }

    func reauthorize() {
        me = nil
        source.reauthorize()
    }

    typealias Row = OfflineStore.Row

    // MARK: Listing

    func list(_ path: LibraryPath) async throws -> [LibraryItem] {
        try await rows(path).map(item)
    }

    /// A listing's rows: from Spotify (and saved), or the saved copy when
    /// offline.
    func rows(_ path: LibraryPath) async throws -> [Row] {
        if path.id == "root" { return Self.root }
        if source.followingApp { return try saved(path) }
        do {
            let rows = try await fetch(path)
            OfflineStore.shared.saveListing(path.id, rows)
            return rows
        } catch where error.isOffline {
            return try saved(path)
        }
    }

    private func saved(_ path: LibraryPath) throws -> [Row] {
        guard let rows = OfflineStore.shared.listing(path.id) else {
            throw LibraryError.message(
                "Not saved for offline yet. Open it once while online, or turn on Save Ahead in Settings.")
        }
        return rows
    }

    private static let root: [Row] = [
        Row(id: "liked", title: "liked songs", opensID: "liked", opensTitle: "liked songs"),
        Row(id: "playlists", title: "playlists", opensID: "playlists", opensTitle: "playlists"),
        Row(id: "albums", title: "albums", opensID: "albums", opensTitle: "albums"),
    ]

    private func fetch(_ path: LibraryPath) async throws -> [Row] {
        switch path.id {
        case "liked":
            let raw = try await all("/me/tracks").compactMap { $0["track"] }
            remember(raw)
            let tracks = raw.compactMap(track)
            let uris = tracks.map(\.uri)
            var rows = [playAll(tracks.count, .tracks(uris))]
            rows += tracks.enumerated().map { i, t in row(t, .tracks(Array(uris[i...]))) }
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
                return Row(
                    id: "album:\(id)", title: name, subtitle: artists(a),
                    detail: (a["total_tracks"] as? Int).map(String.init),
                    opensID: "album:\(id):\(uri)", opensTitle: name)
            }
        default:
            // "playlist:ID:spotify:playlist:ID" or "album:ID:spotify:album:ID".
            let parts = path.id.split(separator: ":", maxSplits: 2).map(String.init)
            guard parts.count == 3 else { return [] }
            let (kind, id, uri) = (parts[0], parts[1], parts[2])
            let raw: [Any]
            if kind == "playlist" {
                raw = try await all("/playlists/\(id)/items").compactMap { e in
                    e["item"] is [String: Any] ? e["item"] : e["track"]
                }
            } else {
                raw = try await all("/albums/\(id)/tracks")
            }
            if kind == "playlist" { remember(raw) }
            let tracks = raw.compactMap(track)
            var rows = [playAll(tracks.count, .context(uri: uri, offset: nil, index: nil))]
            rows += tracks.enumerated().map { i, t in row(t, .context(uri: uri, offset: t.uri, index: i)) }
            return rows
        }
    }

    func search(_ term: String) async throws -> [LibraryItem] {
        if source.followingApp { return savedSearch(term) }
        var c = URLComponents()
        c.queryItems = [
            .init(name: "q", value: term), .init(name: "type", value: "track,album,playlist"),
            .init(name: "limit", value: "10"),
        ]
        let json: [String: Any]
        do {
            json = try await get("/search?\(c.percentEncodedQuery ?? "")")
        } catch where error.isOffline {
            return savedSearch(term)
        }
        func items(_ key: String) -> [[String: Any]] {
            ((json[key] as? [String: Any])?["items"] as? [Any])?.compactMap { $0 as? [String: Any] } ?? []
        }
        let playlists: [Row] = items("playlists").compactMap { p in
            guard let uri = p["uri"] as? String else { return nil }
            let name = p["name"] as? String ?? "Untitled"
            return Row(id: uri, title: name, subtitle: "playlist", action: .context(uri: uri, offset: nil, index: nil))
        }
        let albums: [Row] = items("albums").compactMap { a in
            guard let id = a["id"] as? String, let uri = a["uri"] as? String else { return nil }
            let name = a["name"] as? String ?? "Untitled"
            return Row(
                id: uri, title: name, subtitle: artists(a), opensID: "album:\(id):\(uri)", opensTitle: name)
        }
        let tracks: [Row] = items("tracks").compactMap(track).map { row($0, .tracks([$0.uri])) }
        return (tracks + albums + playlists).map(item)
    }

    /// Offline search: the saved songs.
    private func savedSearch(_ term: String) -> [LibraryItem] {
        OfflineStore.shared.search(term).map { r in
            item(Row(
                id: r.id, title: r.title, subtitle: r.artist,
                detail: r.duration.map { timeText($0) }, action: .tracks([r.id]), trackURI: r.id))
        }
    }

    func upNext() async throws -> QueueSnapshot {
        if source.followingApp {
            let current = source.nowPlaying.map { QueueSnapshot.Entry(id: $0.trackID, title: $0.title, subtitle: $0.artist) }
            return QueueSnapshot(current: current, upcoming: [], note: "Spotify's queue can't be read offline.")
        }
        let json = try await get("/me/player/queue")
        func entry(_ v: Any?) -> QueueSnapshot.Entry? {
            guard let t = track(v) else { return nil }
            return .init(id: UUID().uuidString, title: t.name, subtitle: t.artists)
        }
        let upcoming = (json["queue"] as? [Any] ?? []).prefix(50).compactMap(entry)
        return QueueSnapshot(current: entry(json["currently_playing"]), upcoming: Array(upcoming), note: nil)
    }

    // MARK: Rows

    private func playlistRow(_ p: [String: Any], mine: String) -> Row? {
        guard let id = p["id"] as? String, let uri = p["uri"] as? String else { return nil }
        let name = p["name"] as? String ?? "Untitled"
        let ownerInfo = p["owner"] as? [String: Any] ?? [:]
        let owner = ownerInfo["id"] as? String ?? ""
        let ownerName = ownerInfo["display_name"] as? String ?? owner
        let readable = owner == mine || (p["collaborative"] as? Bool ?? false)
        let items = p["items"] as? [String: Any]
        let tracks = p["tracks"] as? [String: Any]
        let total: Int? = (items?["total"] as? Int) ?? (tracks?["total"] as? Int)
        var row = Row(
            id: "playlist:\(id)", title: name,
            subtitle: owner == mine ? nil : "by \(ownerName)",
            detail: total.map { String($0) })
        if readable {
            row.opensID = "playlist:\(id):\(uri)"
            row.opensTitle = name
        } else {
            // Others' playlists can be played but not listed.
            row.action = .context(uri: uri, offset: nil, index: nil)
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

    /// Saves the tracks' details (cover URL, ISRC) for offline.
    private func remember(_ raw: [Any]) {
        for v in raw {
            guard let t = v as? [String: Any], !(t["is_local"] as? Bool ?? false),
                let uri = t["uri"] as? String, uri.hasPrefix("spotify:track:")
            else { continue }
            let album = t["album"] as? [String: Any] ?? [:]
            let images = (album["images"] as? [[String: Any]]) ?? []
            let cover = images.max { ($0["width"] as? Int ?? 0) < ($1["width"] as? Int ?? 0) }
            OfflineStore.shared.remember(OfflineStore.Record(
                id: uri, title: t["name"] as? String ?? "Untitled", artist: artists(t) ?? "",
                album: album["name"] as? String ?? "",
                duration: (t["duration_ms"] as? Double).map { $0 / 1000 },
                isrc: (t["external_ids"] as? [String: Any])?["isrc"] as? String,
                artworkURL: cover?["url"] as? String))
        }
    }

    private func artists(_ v: [String: Any]) -> String? {
        let names = (v["artists"] as? [Any])?.compactMap { ($0 as? [String: Any])?["name"] as? String } ?? []
        return names.isEmpty ? nil : names.joined(separator: ", ")
    }

    private func row(_ t: Track, _ action: Row.Action) -> Row {
        Row(
            id: UUID().uuidString, title: t.name, subtitle: t.artists,
            detail: t.durationMS.map { timeText(TimeInterval($0) / 1000) },
            action: action, trackURI: t.uri)
    }

    private func playAll(_ n: Int, _ action: Row.Action) -> Row {
        Row(id: "play-all", title: "play all", detail: "\(n)", action: action)
    }

    /// A saved row with its actions.
    private func item(_ r: Row) -> LibraryItem {
        var item = LibraryItem(id: r.id, title: r.title, subtitle: r.subtitle, detail: r.detail)
        if let id = r.opensID { item.opens = LibraryPath(id: id, title: r.opensTitle ?? r.title) }
        switch r.action {
        case .context(let uri, let offset, let index)?:
            item.play = { [weak self] in try await self?.source.play(context: uri, offset: offset, index: index) }
        case .tracks(let uris)?:
            item.play = { [weak self] in try await self?.source.play(uris: uris) }
        case nil:
            break
        }
        if let uri = r.trackURI {
            item.enqueue = { [weak self] _ in try await self?.source.enqueue(uri) }
        }
        return item
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
        let (status, data) = try await source.call(method, path, body: body)
        // 403 on reads: the login lacks the library scopes.
        if status == 403 && method == "GET" { throw LibraryError.needsLogin }
        return (status, data)
    }
}
