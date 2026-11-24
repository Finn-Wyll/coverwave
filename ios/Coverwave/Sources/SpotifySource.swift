import SpotifyiOS
import UIKit

/// Follows and controls the Spotify app through Spotify's iOS SDK (App
/// Remote). The Spotify app must be installed; the SDK talks to it locally.
///
/// Login: `authorizeAndPlayURI` opens Spotify, which asks the user to allow
/// coverwave and returns to `coverwave://spotify-callback` with an access
/// token (`handle(url:)`). The token also works for the Web API, which
/// supplies the genre, ISRC and cover URL (`SpotifyWebAPI`).
///
/// App Remote disconnects when coverwave goes to the background; `resume()`
/// reconnects when it returns.
@MainActor
@Observable
final class SpotifySource: NSObject, MusicSource {
    static let redirectURL = URL(string: "coverwave://spotify-callback")!
    private static let tokenKey = "spotify.access-token"

    let kind = SourceKind.spotify
    private(set) var status: SourceStatus = .disconnected
    private(set) var nowPlaying: NowPlaying?
    @ObservationIgnored var onChange: ((NowPlaying?) -> Void)?

    @ObservationIgnored private let appRemote: SPTAppRemote?
    @ObservationIgnored private var currentTrack: (any SPTAppRemoteTrack)?
    @ObservationIgnored private var details: [String: Task<SpotifyWebAPI.Details?, Never>] = [:]
    @ObservationIgnored private var resolved: [String: SpotifyWebAPI.Details?] = [:]

    /// Whether a client ID is set in `Config/Local.xcconfig`.
    var isConfigured: Bool { appRemote != nil }

    override init() {
        let id = (Bundle.main.object(forInfoDictionaryKey: "SpotifyClientID") as? String ?? "")
            .trimmingCharacters(in: .whitespaces)
        if id.isEmpty || id.hasPrefix("$(") {
            appRemote = nil
        } else {
            let config = SPTConfiguration(clientID: id, redirectURL: Self.redirectURL)
            appRemote = SPTAppRemote(configuration: config, logLevel: .error)
        }
        super.init()
        appRemote?.delegate = self
        appRemote?.connectionParameters.accessToken = Keychain.read(Self.tokenKey)
        if !isConfigured {
            status = .unavailable("Add your Spotify client ID to ios/Config/Local.xcconfig")
        }
    }

    private var token: String? { appRemote?.connectionParameters.accessToken }

    func connect() {
        guard let remote = appRemote else { return }
        if remote.isConnected { return }
        status = .connecting
        if token == nil {
            authorize()
        } else {
            remote.connect()
        }
    }

    func resume() {
        guard let remote = appRemote, token != nil, !remote.isConnected else { return }
        status = .connecting
        remote.connect()
    }

    /// Opens Spotify to approve coverwave. An empty URI resumes whatever was
    /// playing (Spotify starts playback as part of this handshake).
    private func authorize() {
        appRemote?.authorizeAndPlayURI("") { [weak self] installed in
            guard !installed else { return }
            Task { @MainActor in
                self?.status = .unavailable("Install Spotify to use it with coverwave")
            }
        }
    }

    /// Handles the login redirect. Returns false for other URLs.
    func handle(url: URL) -> Bool {
        guard let remote = appRemote, url.scheme == Self.redirectURL.scheme else { return false }
        let params = remote.authorizationParameters(from: url)
        if let token = params?[SPTAppRemoteAccessTokenKey] {
            remote.connectionParameters.accessToken = token
            Keychain.save(token, for: Self.tokenKey)
            remote.connect()
        } else if let message = params?[SPTAppRemoteErrorDescriptionKey] {
            status = .failed(message)
        }
        return true
    }

    func disconnect() {
        appRemote?.disconnect()
        status = .disconnected
        update(nil)
    }

    /// Forgets the saved login.
    func logOut() {
        disconnect()
        appRemote?.connectionParameters.accessToken = nil
        Keychain.delete(Self.tokenKey)
    }

    private func update(_ np: NowPlaying?) {
        if let old = nowPlaying, let np, old.matches(np) { return }
        if nowPlaying == nil, np == nil { return }
        nowPlaying = np
        onChange?(np)
    }

    private func apply(_ snapshot: NowPlaying, track: any SPTAppRemoteTrack) {
        currentTrack = track
        var np = snapshot
        // Genre, ISRC and cover URL come from the Web API, once per track.
        if details[np.trackID] != nil {
            if let d = resolved[np.trackID] ?? nil {
                np.genre = d.genre
                np.isrc = d.isrc
                np.artworkURL = d.artworkURL
            }
        } else if let id = SpotifyWebAPI.trackID(fromURI: np.trackID), let token {
            let uri = np.trackID
            let task = Task { await SpotifyWebAPI.details(trackID: id, token: token) }
            details[uri] = task
            Task {
                resolved[uri] = .some(await task.value)
                if let current = nowPlaying, current.trackID == uri, let track = currentTrack {
                    apply(current, track: track)
                }
            }
        }
        update(np)
    }

    func artwork(for track: NowPlaying) async -> Data? {
        // Wait for the Web API's cover URL: its bytes match the terminal
        // app's, so the cover gets the same seed and motion there and here.
        var url = track.artworkURL
        if url == nil, let pending = details[track.trackID] {
            url = await pending.value?.artworkURL
        }
        if let url,
            let (data, response) = try? await URLSession.shared.data(from: url),
            (response as? HTTPURLResponse)?.statusCode == 200
        {
            return data
        }
        // No Web API details (yet): ask the Spotify app for the image.
        guard let item = currentTrack, item.uri == track.trackID,
            let images = appRemote?.imageAPI
        else { return nil }
        return await withCheckedContinuation { continuation in
            images.fetchImage(forItem: item, with: CGSize(width: 640, height: 640)) { result, _ in
                continuation.resume(returning: (result as? UIImage)?.jpegData(compressionQuality: 0.92))
            }
        }
    }

    func playPause() {
        guard let player = appRemote?.playerAPI else { return }
        if nowPlaying?.isPlaying == true {
            player.pause(nil)
        } else {
            player.resume(nil)
        }
    }

    func next() { appRemote?.playerAPI?.skip(toNext: nil) }

    func previous() { appRemote?.playerAPI?.skip(toPrevious: nil) }

    func seek(to seconds: TimeInterval) {
        appRemote?.playerAPI?.seek(toPosition: Int(seconds * 1000), callback: nil)
    }
}

extension SpotifySource: SPTAppRemoteDelegate, SPTAppRemotePlayerStateDelegate {
    nonisolated func appRemoteDidEstablishConnection(_ appRemote: SPTAppRemote) {
        Task { @MainActor in
            self.status = .connected
            appRemote.playerAPI?.delegate = self
            appRemote.playerAPI?.subscribe(toPlayerState: { _, error in
                guard let error else { return }
                Task { @MainActor in self.status = .failed(error.localizedDescription) }
            })
        }
    }

    nonisolated func appRemote(
        _ appRemote: SPTAppRemote, didFailConnectionAttemptWithError error: Error?
    ) {
        Task { @MainActor in
            // Usually an expired token (they last an hour): the next Connect
            // tap logs in again.
            self.appRemote?.connectionParameters.accessToken = nil
            Keychain.delete(Self.tokenKey)
            self.status = .needsAuthorization
        }
    }

    nonisolated func appRemote(_ appRemote: SPTAppRemote, didDisconnectWithError error: Error?) {
        Task { @MainActor in
            if self.status == .connected { self.status = .disconnected }
        }
    }

    nonisolated func playerStateDidChange(_ playerState: any SPTAppRemotePlayerState) {
        let track = playerState.track
        let snapshot = NowPlaying(
            trackID: track.uri,
            title: track.name,
            artist: track.artist.name,
            album: track.album.name,
            duration: track.duration > 0 ? TimeInterval(track.duration) / 1000 : nil,
            position: TimeInterval(playerState.playbackPosition) / 1000,
            readAt: .now,
            isPlaying: !playerState.isPaused,
            genre: nil,
            isrc: nil,
            artworkURL: nil
        )
        Task { @MainActor in self.apply(snapshot, track: track) }
    }
}
