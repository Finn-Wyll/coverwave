import AuthenticationServices
import CryptoKit
import UIKit

/// Follows and controls Spotify through the Web API, like the terminal app
/// (`crates/cw-sources/src/spotify.rs`).
///
/// Login is Spotify's authorization-code flow with PKCE (no client secret)
/// in an `ASWebAuthenticationSession`, returning to
/// `coverwave://spotify-callback`. The refresh token is kept in the
/// Keychain and access tokens are renewed with it, so one login lasts until
/// you log out. (The Spotify iOS SDK, used before, can only connect while the
/// Spotify app is running, and its tokens expire after an hour: a song left
/// paused, with Spotify suspended, meant logging in again.)
///
/// What is playing comes from polling `GET /me/player` while coverwave is
/// open; it still reports a paused song after iOS suspends Spotify.
/// Controls use the player endpoints (Premium).
///
/// Offline (Offline mode, or whenever the Web API cannot be reached) it
/// follows the Spotify app on this iPhone directly instead
/// (`SpotifyAppLink`), which plays its downloads; what
/// the Web API would add (genre, cover) comes from `OfflineStore`.
@MainActor
@Observable
final class SpotifySource: MusicSource {
    static let redirectURI = "coverwave://spotify-callback"
    static let scopes = [
        "user-read-playback-state", "user-modify-playback-state", "user-read-currently-playing",
        "user-library-read", "playlist-read-private", "playlist-read-collaborative",
        // Following the Spotify app directly while offline.
        "app-remote-control",
    ]
    /// The last Web API access token and its expiry ("token expiry"), for
    /// connecting to the Spotify app while offline.
    private static let accessKey = "spotify.access-token"
    /// The last token the Spotify app accepted (or gave, through
    /// `SpotifyAppLink.authorize`), with when: kept for connecting offline.
    static let appTokenKey = "spotify.app-token"
    /// How often the kept token is renewed quietly while online.
    private static let refreshAppTokenEvery: TimeInterval = 30 * 60
    /// After the Web API could not be reached, follow the app this long
    /// before trying it again.
    private static let retryOnline: TimeInterval = 30
    private static let refreshKey = "spotify.refresh-token"
    private static let api = "https://api.spotify.com/v1"
    private static let accounts = "https://accounts.spotify.com"
    private static let pollInterval: Duration = .seconds(1)

    /// A Spotify Connect device (this iPhone, a computer, a speaker...).
    struct Device: Identifiable, Equatable, Sendable {
        let id: String
        let name: String
        /// Spotify's type: "Smartphone", "Computer", "Speaker"...
        let type: String
        let isActive: Bool

        var isPhone: Bool { type.lowercased() == "smartphone" }
    }

    let kind = SourceKind.spotify
    private(set) var status: SourceStatus = .disconnected
    private(set) var nowPlaying: NowPlaying?
    /// Where Spotify is playing now (from the last poll).
    private(set) var device: Device?
    @ObservationIgnored var onChange: ((NowPlaying?) -> Void)?

    @ObservationIgnored private let clientID: String?
    @ObservationIgnored private var access: (token: String, expires: Date)?
    @ObservationIgnored private var refreshing: Task<String, Error>?
    @ObservationIgnored private var poller: Task<Void, Never>?
    @ObservationIgnored private var login: ASWebAuthenticationSession?
    @ObservationIgnored private let anchor = WindowAnchor()
    /// Genres per artist ID (empty when Spotify has none).
    @ObservationIgnored private var artistGenres: [String: String] = [:]

    /// Offline mode: never use the internet, follow the Spotify app.
    var forceOffline = false {
        didSet { if forceOffline != oldValue { reroute(interactive: false) } }
    }
    /// Following the Spotify app (not the Web API) right now.
    private(set) var followingApp = false
    @ObservationIgnored private var link: SpotifyAppLink?
    @ObservationIgnored private var webDownUntil: Date?
    /// The login lacks `app-remote-control` (made before offline existed).
    private(set) var needsOfflineLogin = false
    @ObservationIgnored private var lastQuietCheck: Date?

    /// Whether to follow the app rather than the Web API.
    var shouldFollowApp: Bool {
        forceOffline || !NetworkMonitor.shared.isOnline || (webDownUntil.map { $0 > .now } ?? false)
    }

    /// Whether a client ID is set in `Config/Local.xcconfig`.
    var isConfigured: Bool { clientID != nil }

    init() {
        let id = (Bundle.main.object(forInfoDictionaryKey: "SpotifyClientID") as? String ?? "")
            .trimmingCharacters(in: .whitespaces)
        clientID = id.isEmpty || id.hasPrefix("$(") ? nil : id
        if !isConfigured {
            status = .unavailable("Add your Spotify client ID to ios/Config/Local.xcconfig")
        }
        needsOfflineLogin = UserDefaults.standard.string(forKey: "spotify.scopes")
            .map { !$0.contains("app-remote-control") } ?? hasLogin
        NetworkMonitor.shared.onChange.append { [weak self] in
            guard let self, self.status != .disconnected else { return }
            if NetworkMonitor.shared.isOnline { self.webDownUntil = nil }
            self.reroute(interactive: false)
        }
    }

    private var hasLogin: Bool { Keychain.read(Self.refreshKey) != nil }

    // MARK: Offline: the Spotify app

    /// Switches between the Web API and the Spotify app as needed.
    private func reroute(interactive: Bool) {
        guard isConfigured, status != .disconnected || interactive else { return }
        if shouldFollowApp {
            guard !followingApp || link?.isConnected != true else { return }
            poller?.cancel()
            poller = nil
            followApp(interactive: interactive)
        } else if followingApp {
            link?.disconnect()
            followingApp = false
            if hasLogin { startPolling() } else { status = .needsAuthorization }
        }
    }

    private func followApp(interactive: Bool) {
        guard let clientID else { return }
        followingApp = true
        let link = self.link ?? makeLink(clientID)
        status = .connecting
        // Authorizing goes through Spotify's servers, so it is only offered
        // when they can be reached: not on Wi-Fi without internet (which
        // iOS still reports as a network) or after the Web API failed.
        let webAnswers = NetworkMonitor.shared.isOnline && webDownUntil == nil
        link.connect(tokens: appTokens(), interactive: interactive && webAnswers)
    }

    private func makeLink(_ clientID: String) -> SpotifyAppLink {
        let link = SpotifyAppLink(clientID: clientID)
        link.onState = { [weak self] state in
            guard let self, self.followingApp else { return }
            switch state {
            case .connected: self.status = .connected
            case .connecting: self.status = .connecting
            case .idle: self.status = .needsAuthorization
            case .failed(let m): self.status = .failed(m)
            }
        }
        link.onConnectedWith = { [weak self] token in
            // Keep what worked, to reconnect quickly.
            Keychain.save("\(token) \(Int(Date().timeIntervalSince1970))", for: Self.appTokenKey)
            if self?.followingApp == false { self?.link?.disconnect() }
        }
        link.onTrack = { [weak self] np in
            guard let self, self.followingApp else { return }
            self.update(OfflineStore.shared.enrich(np))
        }
        self.link = link
        return link
    }

    /// A token for the Spotify app: the Web API's (if it has not expired),
    /// else the last one the app gave, else the Web API's even if expired
    /// (the app may still accept it offline).
    /// Every saved token, best first, without repeats: the player gets
    /// each in turn until one is accepted.
    private func appTokens() -> [String] {
        let saved = Keychain.read(Self.accessKey)?.split(separator: " ").map(String.init).first
        let fromApp = Keychain.read(Self.appTokenKey)?.split(separator: " ").map(String.init).first
        var out: [String] = []
        for t in [appToken(), fromApp, access?.token, saved].compactMap({ $0 }) where !out.contains(t) {
            out.append(t)
        }
        return out
    }

    private func appToken() -> String? {
        let saved = Keychain.read(Self.accessKey)?.split(separator: " ").map(String.init)
        let fromApp = Keychain.read(Self.appTokenKey)?.split(separator: " ").map(String.init)
        // Offline: the token the Spotify app last accepted, whatever its age
        // (the app cannot check it with Spotify's servers either).
        if !NetworkMonitor.shared.isOnline, let fromApp { return fromApp.first }
        // A token the Spotify app took in the last 55 minutes surely works.
        if let fromApp, fromApp.count == 2, let t = Double(fromApp[1]),
            Date().timeIntervalSince1970 - t < 55 * 60
        {
            return fromApp[0]
        }
        if let access, access.expires > .now, !needsOfflineLogin { return access.token }
        if let saved, saved.count == 2, let t = Double(saved[1]), Date(timeIntervalSince1970: t) > .now {
            return saved[0]
        }
        return fromApp?.first ?? saved?.first
    }

    /// Spotify's return from authorizing in its app. False for other URLs.
    func handle(url: URL) -> Bool {
        link?.handle(url: url) ?? false
    }

    /// While online and following the Web API, connects to the Spotify app
    /// in the background now and then (no app switch) to renew the token
    /// kept for offline. Does nothing if Spotify is not running.
    private func renewOfflineToken() {
        guard let clientID, !followingApp, NetworkMonitor.shared.isOnline, !needsOfflineLogin,
            let access, access.expires > .now,
            lastQuietCheck.map({ Date().timeIntervalSince($0) > Self.refreshAppTokenEvery }) ?? true
        else { return }
        lastQuietCheck = .now
        let link = self.link ?? makeLink(clientID)
        guard !link.isBusy else { return }
        link.connect(token: access.token, interactive: false)
    }

    /// The Web API failed to reach Spotify: follow the app for a while.
    private func webUnreachable() {
        webDownUntil = Date().addingTimeInterval(Self.retryOnline)
        reroute(interactive: false)
    }

    // MARK: Connecting

    func connect() {
        guard isConfigured else { return }
        if shouldFollowApp {
            followApp(interactive: true)
        } else if hasLogin {
            startPolling()
        } else {
            logIn()
        }
    }

    func resume() {
        guard isConfigured else { return }
        if shouldFollowApp {
            followApp(interactive: false)
        } else if hasLogin {
            startPolling()
        }
    }

    func disconnect() {
        poller?.cancel()
        poller = nil
        link?.disconnect()
        followingApp = false
        status = .disconnected
        update(nil)
    }

    /// Forgets the saved login.
    func logOut() {
        disconnect()
        access = nil
        Keychain.delete(Self.refreshKey)
    }

    /// Logs in again (from the library's "log in again" row).
    func reauthorize() {
        logOut()
        logIn()
    }

    private func startPolling() {
        guard poller == nil else { return }
        status = .connecting
        poller = Task { [weak self] in
            while !Task.isCancelled {
                await self?.poll()
                try? await Task.sleep(for: Self.pollInterval)
            }
        }
    }

    // MARK: Login (PKCE)

    private func logIn() {
        guard let clientID else { return }
        status = .connecting
        let verifier = Self.randomString(64)
        let challenge = Data(SHA256.hash(data: Data(verifier.utf8))).base64URL
        let state = Self.randomString(16)
        var c = URLComponents(string: "\(Self.accounts)/authorize")!
        c.queryItems = [
            .init(name: "client_id", value: clientID),
            .init(name: "response_type", value: "code"),
            .init(name: "redirect_uri", value: Self.redirectURI),
            .init(name: "code_challenge_method", value: "S256"),
            .init(name: "code_challenge", value: challenge),
            .init(name: "state", value: state),
            .init(name: "scope", value: Self.scopes.joined(separator: " ")),
        ]
        let session = ASWebAuthenticationSession(url: c.url!, callbackURLScheme: "coverwave") {
            [weak self] url, error in
            Task { @MainActor in
                guard let self else { return }
                self.login = nil
                guard let url,
                    let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
                    items.first(where: { $0.name == "state" })?.value == state,
                    let code = items.first(where: { $0.name == "code" })?.value
                else {
                    let cancelled = (error as? ASWebAuthenticationSessionError)?.code == .canceledLogin
                    self.status = cancelled ? .needsAuthorization : .failed("Spotify login failed")
                    return
                }
                do {
                    try await self.exchange(code: code, verifier: verifier)
                    self.startPolling()
                } catch {
                    self.status = .failed(error.localizedDescription)
                }
            }
        }
        session.presentationContextProvider = anchor
        // Keep Spotify's web login, so logging in again is one tap.
        session.prefersEphemeralWebBrowserSession = false
        login = session
        if !session.start() { status = .failed("Could not open the Spotify login") }
    }

    private struct TokenResponse: Decodable {
        let access_token: String
        let expires_in: Double
        let refresh_token: String?
        let scope: String?
    }

    private func exchange(code: String, verifier: String) async throws {
        try await tokenRequest([
            "grant_type": "authorization_code", "code": code,
            "redirect_uri": Self.redirectURI, "code_verifier": verifier,
        ])
    }

    /// Posts to the token endpoint and keeps what comes back.
    @discardableResult
    private func tokenRequest(_ form: [String: String]) async throws -> String {
        guard let clientID else { throw LibraryError.needsLogin }
        var request = URLRequest(url: URL(string: "\(Self.accounts)/api/token")!)
        request.httpMethod = "POST"
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        var body = URLComponents()
        body.queryItems = (form.merging(["client_id": clientID]) { a, _ in a })
            .map { URLQueryItem(name: $0.key, value: $0.value) }
        request.httpBody = Data((body.percentEncodedQuery ?? "").utf8)
        let (data, response) = try await URLSession.shared.data(for: request)
        let code = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard code == 200, let token = try? JSONDecoder().decode(TokenResponse.self, from: data) else {
            // 400 invalid_grant: the refresh token was revoked or expired.
            if code == 400 {
                Keychain.delete(Self.refreshKey)
                throw LibraryError.needsLogin
            }
            throw LibraryError.message("Spotify login: HTTP \(code)")
        }
        let expires = Date().addingTimeInterval(token.expires_in - 60)
        access = (token.access_token, expires)
        Keychain.save("\(token.access_token) \(Int(expires.timeIntervalSince1970))", for: Self.accessKey)
        if let refresh = token.refresh_token { Keychain.save(refresh, for: Self.refreshKey) }
        if let scope = token.scope {
            UserDefaults.standard.set(scope, forKey: "spotify.scopes")
            needsOfflineLogin = !scope.contains("app-remote-control")
        }
        return token.access_token
    }

    /// A valid access token, renewed with the refresh token when needed.
    func accessToken() async throws -> String {
        if let access, access.expires > .now { return access.token }
        if let refreshing { return try await refreshing.value }
        guard let refresh = Keychain.read(Self.refreshKey) else { throw LibraryError.needsLogin }
        let task = Task { try await tokenRequest(["grant_type": "refresh_token", "refresh_token": refresh]) }
        refreshing = task
        defer { refreshing = nil }
        return try await task.value
    }

    // MARK: Web API

    /// One Web API call. Returns the status and body; throws `needsLogin`
    /// when the login is gone.
    @discardableResult
    func call(_ method: String, _ path: String, body: [String: Any]? = nil) async throws -> (Int, Data) {
        var token = try await accessToken()
        for attempt in 0..<2 {
            var request = URLRequest(url: URL(string: Self.api + path)!)
            request.httpMethod = method
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
            if let body {
                request.httpBody = try JSONSerialization.data(withJSONObject: body)
                request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            } else if method != "GET" {
                request.httpBody = Data()
            }
            let (data, response) = try await URLSession.shared.data(for: request)
            let code = (response as? HTTPURLResponse)?.statusCode ?? 0
            if code == 401 && attempt == 0 {
                // Expired early: renew once and retry.
                access = nil
                token = try await accessToken()
                continue
            }
            return (code, data)
        }
        throw LibraryError.needsLogin
    }

    private func json(_ data: Data) -> [String: Any] {
        (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
    }

    // MARK: Following

    private func poll() async {
        guard !followingApp else { return }
        do {
            let (code, data) = try await call("GET", "/me/player?additional_types=episode")
            if status != .connected { status = .connected }
            renewOfflineToken()
            switch code {
            case 200: await apply(json(data))
            case 204:  // nothing playing anywhere
                device = nil
                update(nil)
            case 429: try? await Task.sleep(for: .seconds(5))
            default: break
            }
        } catch LibraryError.needsLogin {
            poller?.cancel()
            poller = nil
            status = .needsAuthorization
        } catch where error.isOffline {
            webUnreachable()
        } catch {
            // Never connected yet: say why instead of "connecting" forever.
            // Once connected, a passing failure keeps the last state.
            if status != .connected {
                status = .failed((error as? LocalizedError)?.errorDescription ?? error.localizedDescription)
            }
        }
    }

    private static func device(_ d: [String: Any]?) -> Device? {
        guard let d, let id = d["id"] as? String else { return nil }
        return Device(
            id: id, name: d["name"] as? String ?? "Unknown device",
            type: d["type"] as? String ?? "", isActive: d["is_active"] as? Bool ?? false)
    }

    private func apply(_ state: [String: Any]) async {
        let playingOn = Self.device(state["device"] as? [String: Any])
        if playingOn != device { device = playingOn }
        guard let item = state["item"] as? [String: Any], let uri = item["uri"] as? String else {
            return update(nil)
        }
        let album = item["album"] as? [String: Any] ?? [:]
        let artists = (item["artists"] as? [[String: Any]]) ?? []
        let names = artists.compactMap { $0["name"] as? String }
        let images = (album["images"] as? [[String: Any]]) ?? []
        let cover = images.max { ($0["width"] as? Int ?? 0) < ($1["width"] as? Int ?? 0) }
        let isrc = (item["external_ids"] as? [String: Any])?["isrc"] as? String
        let artistID = artists.first?["id"] as? String
        var np = NowPlaying(
            trackID: uri,
            title: item["name"] as? String ?? "Unknown title",
            artist: names.joined(separator: ", ")
                .isEmpty ? ((item["show"] as? [String: Any])?["name"] as? String ?? "") : names.joined(separator: ", "),
            album: album["name"] as? String ?? "",
            duration: (item["duration_ms"] as? Double).map { $0 / 1000 },
            position: (state["progress_ms"] as? Double ?? 0) / 1000,
            readAt: .now,
            isPlaying: state["is_playing"] as? Bool ?? false,
            genre: nil,
            isrc: isrc,
            artworkURL: (cover?["url"] as? String).flatMap(URL.init(string:))
        )
        if let artistID {
            if let g = artistGenres[artistID] {
                np.genre = g.isEmpty ? nil : g
            } else {
                // Once per artist; empty for most artists in 2026, in which
                // case AppModel asks Deezer.
                artistGenres[artistID] = ""
                if let (code, data) = try? await call("GET", "/artists/\(artistID)"), code == 200 {
                    let genres = json(data)["genres"] as? [String] ?? []
                    artistGenres[artistID] = genres.joined(separator: ", ")
                    np.genre = genres.isEmpty ? nil : genres.joined(separator: ", ")
                }
            }
        }
        OfflineStore.shared.remember(np)
        update(np)
    }

    private func update(_ np: NowPlaying?) {
        if let old = nowPlaying, let np, old.matches(np) { return }
        if nowPlaying == nil, np == nil { return }
        nowPlaying = np
        onChange?(np)
    }

    /// The saved cover first (the same bytes online and off, so the same
    /// motion), then its URL, then the Spotify app's copy. What is fetched
    /// is saved for offline.
    func artwork(for track: NowPlaying) async -> Data? {
        let store = OfflineStore.shared
        if let saved = store.cover(for: track) { return saved }
        if !followingApp, let url = track.artworkURL,
            let (data, response) = try? await URLSession.shared.data(from: url),
            (response as? HTTPURLResponse)?.statusCode == 200
        {
            store.saveCover(data, key: url.absoluteString)
            return data
        }
        if let data = await link?.image(for: track.trackID) {
            store.saveCover(data, key: track.trackID)
            return data
        }
        return nil
    }

    // MARK: Controls

    func playPause() {
        let playing = nowPlaying?.isPlaying == true
        if var np = nowPlaying {
            // Show it at once; the next poll confirms.
            np.position = np.position(at: .now)
            np.readAt = .now
            np.isPlaying = !playing
            update(np)
        }
        if followingApp {
            link?.playPause(isPlaying: playing)
            return
        }
        control("PUT", playing ? "/me/player/pause" : "/me/player/play")
    }

    func next() {
        if followingApp { link?.next() } else { control("POST", "/me/player/next") }
    }

    func previous() {
        if followingApp { link?.previous() } else { control("POST", "/me/player/previous") }
    }

    func seek(to seconds: TimeInterval) {
        if followingApp {
            link?.seek(to: seconds)
        } else {
            control("PUT", "/me/player/seek?position_ms=\(Int(seconds * 1000))")
        }
    }

    private func control(_ method: String, _ path: String) {
        Task {
            _ = try? await send(method, path, body: nil)
            await poll()
        }
    }

    /// Plays a context (album or playlist), from one of its tracks.
    func play(context: String, offset: String? = nil, index: Int? = nil) async throws {
        if followingApp {
            guard let link else { throw LibraryError.message("Spotify isn't connected.") }
            return try await link.play(context: context, index: index, track: offset)
        }
        var body: [String: Any] = ["context_uri": context]
        if let offset { body["offset"] = ["uri": offset] }
        try await play(body)
    }

    /// Plays tracks in order. Offline the first plays and up to 20 more are
    /// queued after it.
    func play(uris: [String]) async throws {
        guard let first = uris.first else { return }
        if followingApp {
            guard let link else { throw LibraryError.message("Spotify isn't connected.") }
            try await link.play(first)
            for uri in uris.dropFirst().prefix(20) { try? await link.enqueue(uri) }
            return
        }
        try await play(["uris": Array(uris.prefix(100))])
    }

    /// Plays a context or track list (`PUT /me/player/play` with `body`).
    private func play(_ body: [String: Any]) async throws {
        let (code, _) = try await send("PUT", "/me/player/play", body: body)
        switch code {
        case 200..<300: await poll()
        case 403: throw LibraryError.message("Spotify Premium is needed to start playback from here.")
        default: throw LibraryError.message("Spotify said \(code).")
        }
    }

    /// Adds a track to the end of Spotify's queue.
    func enqueue(_ uri: String) async throws {
        if followingApp {
            guard let link else { throw LibraryError.message("Spotify isn't connected.") }
            return try await link.enqueue(uri)
        }
        let escaped = uri.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? uri
        let (code, _) = try await send("POST", "/me/player/queue?uri=\(escaped)", body: nil)
        guard (200..<300).contains(code) else { throw LibraryError.message("Spotify said \(code).") }
    }

    /// A player call; with no active device (404), wakes Spotify on this
    /// iPhone, moves playback to it and tries once more.
    private func send(_ method: String, _ path: String, body: [String: Any]?) async throws -> (Int, Data) {
        let first = try await call(method, path, body: body)
        guard first.0 == 404 else { return first }
        guard let device = await wakeDevice() else {
            throw LibraryError.message("Open Spotify on this iPhone, then try again.")
        }
        let sep = path.contains("?") ? "&" : "?"
        return try await call(method, "\(path)\(sep)device_id=\(device)", body: body)
    }

    /// A device to play on: an active one, else this iPhone's Spotify
    /// (opened if needed, then coverwave comes back to the front by itself
    /// once the user returns).
    private func wakeDevice() async -> String? {
        if let id = await pickDevice() { return id }
        if let url = URL(string: "spotify:"), UIApplication.shared.canOpenURL(url) {
            await UIApplication.shared.open(url)
        }
        for _ in 0..<10 {
            try? await Task.sleep(for: .seconds(1))
            if let id = await pickDevice() { return id }
        }
        return nil
    }

    // MARK: Devices

    /// Every device Spotify can play on now. This iPhone is listed only
    /// while its Spotify app is awake.
    func devices() async throws -> [Device] {
        if followingApp {
            throw LibraryError.message("Devices need the internet. Offline, Spotify plays on this iPhone.")
        }
        let (code, data) = try await call("GET", "/me/player/devices")
        guard code == 200 else { throw LibraryError.message("Spotify said \(code).") }
        return (json(data)["devices"] as? [[String: Any]] ?? []).compactMap { Self.device($0) }
    }

    /// Moves playback to `device`, keeping it playing (or paused) as it was.
    func transfer(to device: Device) async throws {
        let playing = nowPlaying?.isPlaying ?? false
        let (code, _) = try await call(
            "PUT", "/me/player", body: ["device_ids": [device.id], "play": playing])
        guard (200..<300).contains(code) else {
            throw LibraryError.message(code == 404 ? "That device went away." : "Spotify said \(code).")
        }
        await poll()
    }

    /// Moves playback to this iPhone, waking its Spotify app if it is not
    /// listed (asleep).
    func transferToThisPhone() async throws {
        if let phone = try await devices().first(where: \.isPhone) {
            return try await transfer(to: phone)
        }
        if let url = URL(string: "spotify:"), UIApplication.shared.canOpenURL(url) {
            await UIApplication.shared.open(url)
        }
        for _ in 0..<10 {
            try? await Task.sleep(for: .seconds(1))
            if let phone = try? await devices().first(where: \.isPhone) {
                return try await transfer(to: phone)
            }
        }
        throw LibraryError.message("Spotify on this iPhone did not wake up. Open it, then try again.")
    }

    private func pickDevice() async -> String? {
        guard let (code, data) = try? await call("GET", "/me/player/devices"), code == 200 else { return nil }
        let devices = json(data)["devices"] as? [[String: Any]] ?? []
        let active = devices.first { $0["is_active"] as? Bool == true }
        let phone = devices.first { ($0["type"] as? String)?.lowercased() == "smartphone" }
        return (active ?? phone ?? devices.first)?["id"] as? String
    }

    // MARK: Helpers

    private static func randomString(_ n: Int) -> String {
        let chars = Array("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")
        return String((0..<n).map { _ in chars.randomElement()! })
    }
}

private extension Data {
    var base64URL: String {
        base64EncodedString().replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
    }
}

/// Where the login sheet is shown: the app's key window.
private final class WindowAnchor: NSObject, ASWebAuthenticationPresentationContextProviding {
    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        MainActor.assumeIsolated {
            UIApplication.shared.connectedScenes
                .compactMap { $0 as? UIWindowScene }
                .flatMap(\.windows)
                .first { $0.isKeyWindow } ?? ASPresentationAnchor()
        }
    }
}
