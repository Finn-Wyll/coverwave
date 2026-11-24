import Foundation
import MediaPlayer
import MusicKit
import StoreKit

/// The Music library on this iPhone, through MusicKit. Library requests need
/// only the user's permission (no MusicKit developer service), so this works
/// with a free developer account.
///
/// The system player does not let other apps read its queue, so "up next"
/// lists only what was queued from coverwave this session.
@MainActor
final class AppleMusicLibrary: LibraryProvider {
    let name = "apple music"
    let queuePositions: [QueuePosition] = [.next, .last]

    private var playlists: [String: Playlist] = [:]
    private var albums: [String: Album] = [:]
    /// Queued from coverwave, newest last, as (title, artist).
    private var queued: [(id: String, title: String, artist: String)] = []

    private var player: SystemMusicPlayer { .shared }

    func list(_ path: LibraryPath) async throws -> [LibraryItem] {
        try await authorize()
        switch path.id {
        case "root":
            return [
                folder("playlists", "playlists"),
                folder("albums", "albums"),
                folder("songs", "songs"),
            ]
        case "playlists":
            var request = MusicLibraryRequest<Playlist>()
            request.limit = 500
            let found = try await request.response().items
            return found.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
                .map(playlistRow)
        case "albums":
            var request = MusicLibraryRequest<Album>()
            request.limit = 1000
            let found = try await request.response().items
            return found.sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending }
                .map(albumRow)
        case "songs":
            var request = MusicLibraryRequest<Song>()
            request.limit = 1000
            let songs = try await request.response().items
                .sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending }
            return songs.map { songRow($0, from: songs) }
        default:
            let parts = path.id.split(separator: ":", maxSplits: 1).map(String.init)
            guard parts.count == 2 else { return [] }
            switch parts[0] {
            case "playlist":
                guard let p = playlists[parts[1]] else { return [] }
                let full = try await p.with([.tracks])
                return trackRows(Array(full.tracks ?? []))
            case "album":
                guard let a = albums[parts[1]] else { return [] }
                let full = try await a.with([.tracks])
                return trackRows(Array(full.tracks ?? []))
            case "catalog-album":
                return try await catalogAlbum(parts[1])
            default:
                return []
            }
        }
    }

    /// Your library first, then the whole Apple Music catalogue.
    func search(_ term: String) async throws -> [LibraryItem] {
        try await authorize()
        var request = MusicLibrarySearchRequest(term: term, types: [Song.self, Album.self, Playlist.self])
        request.limit = 10
        let r = try await request.response()
        let songs = Array(r.songs)
        let mine = r.playlists.map(playlistRow) + r.albums.map(albumRow) + songs.map { songRow($0, from: songs) }
        let catalog = (try? await catalogSearch(term)) ?? []
        return mine + catalog
    }

    func upNext() async throws -> QueueSnapshot {
        let current = player.queue.currentEntry.map {
            QueueSnapshot.Entry(id: $0.id, title: $0.title, subtitle: $0.subtitle)
        }
        return QueueSnapshot(
            current: current,
            upcoming: queued.map { .init(id: $0.id, title: $0.title, subtitle: $0.artist) },
            note: "Music doesn't share its queue with other apps: this shows what you added here.")
    }

    func reauthorize() {
        Task { _ = await MusicAuthorization.request() }
    }

    // MARK: Rows

    private func authorize() async throws {
        if MusicAuthorization.currentStatus == .authorized { return }
        guard await MusicAuthorization.request() == .authorized else {
            throw LibraryError.message("Allow coverwave in Settings > Privacy > Media & Apple Music.")
        }
    }

    private func folder(_ id: String, _ title: String) -> LibraryItem {
        LibraryItem(id: id, title: title, opens: LibraryPath(id: id, title: title))
    }

    private func playlistRow(_ p: Playlist) -> LibraryItem {
        let key = p.id.rawValue
        playlists[key] = p
        return LibraryItem(
            id: "playlist:\(key)", title: p.name, subtitle: p.curatorName,
            opens: LibraryPath(id: "playlist:\(key)", title: p.name))
    }

    private func albumRow(_ a: Album) -> LibraryItem {
        let key = a.id.rawValue
        albums[key] = a
        return LibraryItem(
            id: "album:\(key)", title: a.title, subtitle: a.artistName,
            detail: a.trackCount > 0 ? "\(a.trackCount)" : nil,
            opens: LibraryPath(id: "album:\(key)", title: a.title))
    }

    private func songRow(_ song: Song, from list: [Song]) -> LibraryItem {
        LibraryItem(
            id: "song:\(song.id.rawValue)", title: song.title, subtitle: song.artistName,
            detail: song.duration.map(timeText),
            play: { [weak self] in try await self?.start(list, at: song) },
            enqueue: { [weak self] position in
                try await self?.insert(song, title: song.title, artist: song.artistName, position)
            })
    }

    /// Each track plays from itself through the rest of the list.
    private func trackRows(_ tracks: [Track]) -> [LibraryItem] {
        var rows: [LibraryItem] = []
        if let first = tracks.first {
            rows.append(
                LibraryItem(
                    id: "play-all", title: "play all", detail: "\(tracks.count)",
                    play: { [weak self] in try await self?.start(tracks, at: first) }))
        }
        rows += tracks.map { track in
            LibraryItem(
                id: "track:\(track.id.rawValue)", title: track.title, subtitle: track.artistName,
                detail: track.duration.map(timeText),
                play: { [weak self] in try await self?.start(tracks, at: track) },
                enqueue: { [weak self] position in
                    try await self?.insert(track, title: track.title, artist: track.artistName, position)
                })
        }
        return rows
    }

    // MARK: Catalogue

    /// Songs and albums from Apple's public search API (no developer token,
    /// so it works with a free developer account). Sends the search text and
    /// the storefront's country code to itunes.apple.com.
    private struct ITunesResult: Decodable {
        let wrapperType: String?
        let kind: String?
        let trackId: Int?
        let collectionId: Int?
        let trackName: String?
        let collectionName: String?
        let artistName: String?
        let trackTimeMillis: Int?
        let trackCount: Int?
    }

    private struct ITunesResponse: Decodable {
        let results: [ITunesResult]
    }

    private var storefront: String?

    /// The user's Apple Music country, so store IDs play in their region.
    private func country() async -> String {
        if let storefront { return storefront }
        let code = await withCheckedContinuation { (c: CheckedContinuation<String?, Never>) in
            SKCloudServiceController().requestStorefrontCountryCode { code, _ in c.resume(returning: code) }
        }
        // The storefront code is a 2-letter region (or 3-letter on some
        // systems); fall back to the device region.
        let found = code.map { String($0.prefix(2)).lowercased() }
            ?? Locale.current.region?.identifier.lowercased() ?? "us"
        storefront = found
        return found
    }

    private func itunes(_ path: String, _ query: [URLQueryItem]) async throws -> [ITunesResult] {
        var c = URLComponents(string: "https://itunes.apple.com/\(path)")!
        c.queryItems = query + [URLQueryItem(name: "country", value: await country())]
        guard let url = c.url else { return [] }
        let (data, response) = try await URLSession.shared.data(from: url)
        guard (response as? HTTPURLResponse)?.statusCode == 200 else {
            throw LibraryError.message("Apple Music search is unavailable right now.")
        }
        return try JSONDecoder().decode(ITunesResponse.self, from: data).results
    }

    private func catalogSearch(_ term: String) async throws -> [LibraryItem] {
        async let songs = itunes("search", [
            .init(name: "term", value: term), .init(name: "media", value: "music"),
            .init(name: "entity", value: "song"), .init(name: "limit", value: "15"),
        ])
        async let albums = itunes("search", [
            .init(name: "term", value: term), .init(name: "media", value: "music"),
            .init(name: "entity", value: "album"), .init(name: "limit", value: "8"),
        ])
        let songRows = try await songs.compactMap { r -> LibraryItem? in
            guard let id = r.trackId else { return nil }
            return catalogSongRow(r, id: String(id), queue: [String(id)])
        }
        let albumRows = try await albums.compactMap { r -> LibraryItem? in
            guard let id = r.collectionId, let name = r.collectionName else { return nil }
            return LibraryItem(
                id: "catalog-album:\(id)", title: name,
                subtitle: r.artistName.map { "album, \($0)" },
                detail: r.trackCount.map(String.init),
                opens: LibraryPath(id: "catalog-album:\(id)", title: name))
        }
        return songRows + albumRows
    }

    private func catalogAlbum(_ id: String) async throws -> [LibraryItem] {
        let tracks = try await itunes("lookup", [
            .init(name: "id", value: id), .init(name: "entity", value: "song"),
        ]).filter { $0.wrapperType == "track" && $0.trackId != nil }
        let ids = tracks.compactMap { $0.trackId.map(String.init) }
        var rows: [LibraryItem] = []
        if let first = ids.first {
            rows.append(
                LibraryItem(
                    id: "play-all", title: "play all", detail: "\(ids.count)",
                    play: { [weak self] in try await self?.startStore(ids, at: first) }))
        }
        rows += tracks.enumerated().compactMap { i, r in
            guard r.trackId != nil else { return nil }
            return catalogSongRow(r, id: ids[i], queue: ids)
        }
        return rows
    }

    private func catalogSongRow(_ r: ITunesResult, id: String, queue: [String]) -> LibraryItem {
        let title = r.trackName ?? "Untitled"
        let artist = r.artistName ?? ""
        return LibraryItem(
            id: "catalog:\(id)", title: title, subtitle: artist,
            detail: r.trackTimeMillis.map { timeText(TimeInterval($0) / 1000) },
            play: { [weak self] in try await self?.startStore(queue, at: id) },
            enqueue: { [weak self] position in
                self?.insertStore(id, title: title, artist: artist, position)
            })
    }

    /// Plays catalogue songs by store ID through the Music app's player.
    private func startStore(_ ids: [String], at first: String) async throws {
        let system = MPMusicPlayerController.systemMusicPlayer
        let descriptor = MPMusicPlayerStoreQueueDescriptor(storeIDs: ids)
        descriptor.startItemID = first
        system.setQueue(with: descriptor)
        system.play()
        queued.removeAll()
    }

    private func insertStore(_ id: String, title: String, artist: String, _ position: QueuePosition) {
        let system = MPMusicPlayerController.systemMusicPlayer
        let descriptor = MPMusicPlayerStoreQueueDescriptor(storeIDs: [id])
        if position == .next { system.prepend(descriptor) } else { system.append(descriptor) }
        let entry = (id: UUID().uuidString, title: title, artist: artist)
        if position == .next { queued.insert(entry, at: 0) } else { queued.append(entry) }
    }

    // MARK: Playback

    private func start<T: PlayableMusicItem>(_ items: [T], at item: T) async throws {
        player.queue = SystemMusicPlayer.Queue(for: items, startingAt: item)
        try await player.play()
        queued.removeAll()
    }

    private func insert<T: PlayableMusicItem>(
        _ item: T, title: String, artist: String, _ position: QueuePosition
    ) async throws {
        try await player.queue.insert(item, position: position == .next ? .afterCurrentEntry : .tail)
        let entry = (id: UUID().uuidString, title: title, artist: artist)
        if position == .next { queued.insert(entry, at: 0) } else { queued.append(entry) }
    }
}
