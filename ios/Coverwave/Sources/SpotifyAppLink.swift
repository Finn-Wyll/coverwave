import SpotifyiOS
import UIKit

/// A direct line to the Spotify app on this iPhone (Spotify's iOS SDK, App
/// Remote), for when the Web API cannot be reached: it talks to the app on
/// the phone itself, so it follows and controls songs Spotify plays from
/// its downloads, with no internet.
///
/// The Spotify app must be running. Connecting needs an access token with
/// the `app-remote-control` scope: the Web API login's own (kept from the
/// last time online), else one from the Spotify app itself
/// (`authorize()`, which opens Spotify and returns to
/// `coverwave://spotify-callback`).
@MainActor
final class SpotifyAppLink: NSObject {
    enum State: Equatable {
        case idle
        case connecting
        case connected
        /// Connecting failed; a user action can authorize through Spotify.
        case failed(String)
    }

    static let redirectURL = URL(string: "coverwave://spotify-callback")!
    static let notPrepared =
        "Spotify needs coverwave authorised while online first: connect to the internet, then Settings > Offline > Prepare for Offline."

    /// Whether a connection is up or being made.
    var isBusy: Bool { remote.isConnected || state == .connecting }

    private(set) var state: State = .idle {
        didSet { if state != oldValue { onState?(state) } }
    }
    var onState: ((State) -> Void)?
    /// Each player state the app reports.
    var onTrack: ((NowPlaying) -> Void)?
    /// The token a connection succeeded with (to keep for offline).
    var onConnectedWith: ((String) -> Void)?

    private let remote: SPTAppRemote
    private var currentTrack: (any SPTAppRemoteTrack)?
    /// Authorize through the Spotify app if the token is refused.
    private var authorizeOnFailure = false
    /// Gives up on a connection attempt that never answers.
    private var watchdog: Task<Void, Never>?

    /// Fails a connection attempt still pending after `seconds`, so the
    /// screen never just says "connecting".
    private func watch(_ seconds: Double, _ message: String) {
        watchdog?.cancel()
        watchdog = Task { [weak self] in
            try? await Task.sleep(for: .seconds(seconds))
            guard let self, !Task.isCancelled, self.state == .connecting else { return }
            if self.remote.isConnected { return self.state = .connected }
            self.remote.disconnect()
            self.state = .failed(message)
        }
    }

    init(clientID: String) {
        let config = SPTConfiguration(clientID: clientID, redirectURL: Self.redirectURL)
        remote = SPTAppRemote(configuration: config, logLevel: .error)
        super.init()
        remote.delegate = self
    }

    var isConnected: Bool { remote.isConnected }

    /// Connects with `token`. With `interactive`, a refused or missing
    /// token opens Spotify to authorize instead.
    func connect(token: String?, interactive: Bool) {
        if remote.isConnected { return state = .connected }
        // Authorizing goes through Spotify's servers: pointless offline.
        authorizeOnFailure = interactive && NetworkMonitor.shared.isOnline
        guard let token else {
            if authorizeOnFailure {
                authorize()
            } else {
                state = .failed(Self.notPrepared)
            }
            return
        }
        state = .connecting
        remote.connectionParameters.accessToken = token
        remote.connect()
        watch(
            10,
            NetworkMonitor.shared.isOnline
                ? "Spotify didn't answer. Open Spotify and play something, then tap offline again."
                : "Spotify didn't answer. Open Spotify and play a downloaded song, then tap offline again. If it keeps failing, prepare offline again while online.")
    }

    /// Opens the Spotify app, which asks to allow coverwave (once) and
    /// returns with a token. An empty URI resumes what was playing.
    func authorize() {
        state = .connecting
        authorizeOnFailure = false
        watch(90, "Spotify didn't send coverwave back a login. Tap offline to try again.")
        remote.authorizeAndPlayURI("", asRadio: false, additionalScopes: nil) { [weak self] opened in
            guard !opened else { return }
            Task { @MainActor in self?.state = .failed("Install Spotify to use it offline") }
        }
    }

    /// Handles Spotify's return from `authorize()`. False for other URLs.
    func handle(url: URL) -> Bool {
        guard url.scheme == Self.redirectURL.scheme, url.host() == Self.redirectURL.host() else { return false }
        let params = remote.authorizationParameters(from: url)
        if let token = params?[SPTAppRemoteAccessTokenKey] {
            // Kept with when it was given: it lasts about an hour.
            Keychain.save("\(token) \(Int(Date().timeIntervalSince1970))", for: SpotifySource.appTokenKey)
            remote.connectionParameters.accessToken = token
            state = .connecting
            // Spotify is still handing back to coverwave: connect once
            // coverwave is active again, or it can hang.
            Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(600))
                guard let self, !self.remote.isConnected else { return }
                self.remote.connect()
                self.watch(10, "Spotify authorised coverwave but didn't connect. Open Spotify, play something, then tap offline again.")
            }
        } else if let message = params?[SPTAppRemoteErrorDescriptionKey] {
            state = .failed(message)
        } else {
            return false
        }
        return true
    }

    func disconnect() {
        watchdog?.cancel()
        if remote.isConnected { remote.disconnect() }
        currentTrack = nil
        state = .idle
    }

    // MARK: Controls

    func playPause(isPlaying: Bool) {
        if isPlaying { remote.playerAPI?.pause(nil) } else { remote.playerAPI?.resume(nil) }
    }

    func next() { remote.playerAPI?.skip(toNext: nil) }
    func previous() { remote.playerAPI?.skip(toPrevious: nil) }

    func seek(to seconds: TimeInterval) {
        remote.playerAPI?.seek(toPosition: Int(seconds * 1000), callback: nil)
    }

    /// Plays a track, album or playlist URI.
    func play(_ uri: String) async throws {
        try await call { api, done in api.play(uri, callback: done) }
    }

    /// Plays a context from its track at `index` (falls back to the track,
    /// or the whole context, when Spotify cannot open it).
    func play(context: String, index: Int?, track: String?) async throws {
        if let index, let content = remote.contentAPI, let api = remote.playerAPI, remote.isConnected {
            let item: (any SPTAppRemoteContentItem)? = await withCheckedContinuation { c in
                content.fetchContentItem(forURI: context) { result, _ in
                    c.resume(returning: result as? any SPTAppRemoteContentItem)
                }
            }
            if let item {
                do {
                    return try await withCheckedThrowingContinuation { (c: CheckedContinuation<Void, Error>) in
                        api.play(item, skipToTrackIndex: index) { _, error in
                            if let error { c.resume(throwing: error) } else { c.resume() }
                        }
                    }
                } catch {}
            }
        }
        try await play(track ?? context)
    }

    func enqueue(_ uri: String) async throws {
        try await call { api, done in api.enqueueTrackUri(uri, callback: done) }
    }

    private func call(
        _ body: (any SPTAppRemotePlayerAPI, @escaping SPTAppRemoteCallback) -> Void
    ) async throws {
        guard let api = remote.playerAPI, remote.isConnected else {
            throw LibraryError.message("Spotify isn't connected. Open Spotify, then try again.")
        }
        try await withCheckedThrowingContinuation { (c: CheckedContinuation<Void, Error>) in
            body(api) { _, error in
                if let error { c.resume(throwing: error) } else { c.resume() }
            }
        }
    }

    /// The playing track's cover, from the Spotify app's own copy.
    func image(for trackID: String) async -> Data? {
        guard let item = currentTrack, item.uri == trackID, let images = remote.imageAPI else { return nil }
        return await withCheckedContinuation { c in
            images.fetchImage(forItem: item, with: CGSize(width: 640, height: 640)) { result, _ in
                c.resume(returning: (result as? UIImage)?.jpegData(compressionQuality: 0.92))
            }
        }
    }

    fileprivate func received(_ np: NowPlaying, track: any SPTAppRemoteTrack) {
        currentTrack = track
        onTrack?(np)
    }

    fileprivate func connectionFailed(_ message: String) {
        if authorizeOnFailure {
            authorize()
        } else {
            state = .failed(message)
        }
    }
}

extension SpotifyAppLink: SPTAppRemoteDelegate, SPTAppRemotePlayerStateDelegate {
    nonisolated func appRemoteDidEstablishConnection(_ appRemote: SPTAppRemote) {
        Task { @MainActor in
            self.watchdog?.cancel()
            self.state = .connected
            if let token = appRemote.connectionParameters.accessToken { self.onConnectedWith?(token) }
            appRemote.playerAPI?.delegate = self
            appRemote.playerAPI?.subscribe(toPlayerState: { _, error in
                guard let error else { return }
                Task { @MainActor in self.state = .failed(error.localizedDescription) }
            })
        }
    }

    nonisolated func appRemote(_ appRemote: SPTAppRemote, didFailConnectionAttemptWithError error: Error?) {
        Task { @MainActor in
            self.watchdog?.cancel()
            self.connectionFailed(
                NetworkMonitor.shared.isOnline
                    ? "Spotify refused the connection (\(error?.localizedDescription ?? "no reason given")). Tap to authorise again."
                    : "Spotify refused the saved login offline. Open Spotify and play a downloaded song, then try again; if it still fails, prepare offline again while online.")
        }
    }

    nonisolated func appRemote(_ appRemote: SPTAppRemote, didDisconnectWithError error: Error?) {
        Task { @MainActor in
            if self.state == .connected { self.state = .idle }
        }
    }

    nonisolated func playerStateDidChange(_ playerState: any SPTAppRemotePlayerState) {
        let track = playerState.track
        let np = NowPlaying(
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
        Task { @MainActor in self.received(np, track: track) }
    }
}
