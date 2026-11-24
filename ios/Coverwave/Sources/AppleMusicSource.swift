import MediaPlayer
import MusicKit
import UIKit

/// Follows and controls the Music app through the system music player.
///
/// `MPMusicPlayerController.systemMusicPlayer` gives the state and the
/// transport. MusicKit's catalog adds what it lacks: a downloadable cover,
/// genres and the ISRC (for the tempo lookup). Library-only tracks fall back
/// to their embedded artwork and genre tag.
@MainActor
@Observable
final class AppleMusicSource: MusicSource {
    let kind = SourceKind.appleMusic
    private(set) var status: SourceStatus = .disconnected
    private(set) var nowPlaying: NowPlaying?
    @ObservationIgnored var onChange: ((NowPlaying?) -> Void)?

    private struct CatalogInfo {
        var artworkURL: URL?
        var genre: String?
        var isrc: String?
    }

    @ObservationIgnored private let player = MPMusicPlayerController.systemMusicPlayer
    @ObservationIgnored private var observers: [NSObjectProtocol] = []
    @ObservationIgnored private var resync: Task<Void, Never>?
    @ObservationIgnored private var lookups: [String: Task<CatalogInfo?, Never>] = [:]
    @ObservationIgnored private var catalog: [String: CatalogInfo] = [:]

    func connect() {
        status = .connecting
        Task {
            switch await MusicAuthorization.request() {
            case .authorized: start()
            case .denied, .restricted:
                status = .unavailable("Allow coverwave in Settings › Privacy › Media & Apple Music")
            default: status = .needsAuthorization
            }
        }
    }

    func resume() {
        if MusicAuthorization.currentStatus == .authorized { start() }
    }

    private func start() {
        guard observers.isEmpty else { return refresh() }
        status = .connected
        player.beginGeneratingPlaybackNotifications()
        let names: [Notification.Name] = [
            .MPMusicPlayerControllerNowPlayingItemDidChange,
            .MPMusicPlayerControllerPlaybackStateDidChange,
        ]
        for name in names {
            observers.append(
                NotificationCenter.default.addObserver(
                    forName: name, object: player, queue: .main
                ) { [weak self] _ in
                    MainActor.assumeIsolated { self?.refresh() }
                })
        }
        // Seeks made in Music post no notification: re-read now and then.
        resync = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(2))
                self?.refresh()
            }
        }
        refresh()
    }

    func disconnect() {
        observers.forEach(NotificationCenter.default.removeObserver)
        observers.removeAll()
        player.endGeneratingPlaybackNotifications()
        resync?.cancel()
        status = .disconnected
        update(nil)
    }

    private func refresh() {
        guard status == .connected, let item = player.nowPlayingItem else {
            return update(nil)
        }
        let storeID = item.playbackStoreID
        let inCatalog = !storeID.isEmpty && storeID != "0"
        let id = inCatalog ? storeID : String(item.persistentID)
        let info = catalog[id]
        let time = player.currentPlaybackTime
        let np = NowPlaying(
            trackID: "am:\(id)",
            title: item.title ?? "Unknown title",
            artist: item.artist ?? "",
            album: item.albumTitle ?? "",
            duration: item.playbackDuration > 0 ? item.playbackDuration : nil,
            position: time.isFinite ? time : 0,
            readAt: .now,
            isPlaying: player.playbackState == .playing,
            genre: info?.genre ?? item.genre,
            isrc: info?.isrc,
            artworkURL: info?.artworkURL
        )
        update(np)
        if inCatalog, lookups[id] == nil {
            let task = lookUp(storeID: id)
            Task {
                catalog[id] = await task.value ?? CatalogInfo()
                refresh()
            }
        }
    }

    private func update(_ np: NowPlaying?) {
        // Every read moves `readAt`; only report real changes.
        if let old = nowPlaying, let np, old.matches(np) { return }
        if nowPlaying == nil, np == nil { return }
        nowPlaying = np
        onChange?(np)
    }

    private func lookUp(storeID: String) -> Task<CatalogInfo?, Never> {
        if let task = lookups[storeID] { return task }
        let task = Task<CatalogInfo?, Never> {
            let request = MusicCatalogResourceRequest<Song>(
                matching: \.id, equalTo: MusicItemID(storeID))
            guard let song = try? await request.response().items.first else { return nil }
            let genres = song.genreNames.filter { $0 != "Music" }
            return CatalogInfo(
                artworkURL: song.artwork?.url(width: 640, height: 640),
                genre: genres.isEmpty ? nil : genres.joined(separator: ", "),
                isrc: song.isrc
            )
        }
        lookups[storeID] = task
        return task
    }

    func artwork(for track: NowPlaying) async -> Data? {
        let id = String(track.trackID.dropFirst(3))
        if let url = await lookups[id]?.value?.artworkURL,
            let (data, response) = try? await URLSession.shared.data(from: url),
            (response as? HTTPURLResponse)?.statusCode == 200
        {
            return data
        }
        // Library-only tracks: their embedded artwork.
        guard nowPlaying?.trackID == track.trackID,
            let image = player.nowPlayingItem?.artwork?.image(at: CGSize(width: 640, height: 640))
        else { return nil }
        return image.pngData()
    }

    func playPause() {
        if player.playbackState == .playing { player.pause() } else { player.play() }
        refresh()
    }

    func next() { player.skipToNextItem() }

    func previous() {
        // Like Music: restart the song unless it has only just begun.
        if player.currentPlaybackTime > 3 {
            player.skipToBeginning()
        } else {
            player.skipToPreviousItem()
        }
    }

    func seek(to seconds: TimeInterval) {
        player.currentPlaybackTime = seconds
        refresh()
    }
}
