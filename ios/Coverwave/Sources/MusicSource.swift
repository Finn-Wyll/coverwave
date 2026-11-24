import Foundation

/// Where the music plays. coverwave never plays audio itself on iOS: it
/// follows and controls another app.
enum SourceKind: String, CaseIterable, Identifiable {
    case spotify
    case appleMusic

    var id: String { rawValue }

    var name: String {
        switch self {
        case .spotify: "Spotify"
        case .appleMusic: "Apple Music"
        }
    }

    var symbol: String {
        switch self {
        case .spotify: "headphones"
        case .appleMusic: "music.note"
        }
    }
}

/// The track a source is playing, as last read. The position is
/// extrapolated between reads with `position(at:)`.
struct NowPlaying: Equatable, Sendable {
    /// Stable per track (a Spotify URI, or `am:<store id>`).
    var trackID: String
    var title: String
    var artist: String
    var album: String
    var duration: TimeInterval?
    var position: TimeInterval
    var readAt: Date
    var isPlaying: Bool
    /// Raw genre tag(s), comma-separated. The Rust core maps them to a
    /// family.
    var genre: String?
    /// For the tempo lookup.
    var isrc: String?
    /// Cover URL whose bytes match what the terminal app downloads, so both
    /// apps give a cover the same seed and motion.
    var artworkURL: URL?

    func position(at date: Date) -> TimeInterval {
        guard isPlaying else { return position }
        let p = position + max(0, date.timeIntervalSince(readAt))
        return duration.map { min(p, $0) } ?? p
    }

    /// The same track and state, with the position where it was expected
    /// to be. Sources use this to skip reporting reads that change nothing.
    func matches(_ other: NowPlaying) -> Bool {
        let drift = abs(position(at: other.readAt) - other.position)
        var a = self
        var b = other
        (a.position, a.readAt) = (0, .distantPast)
        (b.position, b.readAt) = (0, .distantPast)
        return a == b && drift < 0.5
    }
}

enum SourceStatus: Equatable {
    case disconnected
    case connecting
    case connected
    /// The user has to approve access first (a tap on Connect).
    case needsAuthorization
    case unavailable(String)
    case failed(String)

    var message: String? {
        switch self {
        case .unavailable(let m), .failed(let m): m
        case .needsAuthorization: "Needs your permission"
        case .connecting: "Connecting…"
        default: nil
        }
    }
}

@MainActor
protocol MusicSource: AnyObject {
    var kind: SourceKind { get }
    var status: SourceStatus { get }
    var nowPlaying: NowPlaying? { get }
    /// Called on the main actor whenever `nowPlaying` changes.
    var onChange: ((NowPlaying?) -> Void)? { get set }

    /// Starts following the source. May ask the user for permission, so
    /// call it from a user action the first time.
    func connect()
    /// Reconnects quietly if the user already gave permission.
    func resume()
    func disconnect()

    /// The cover image's bytes (JPEG or PNG), or nil.
    func artwork(for track: NowPlaying) async -> Data?

    func playPause()
    func next()
    func previous()
    func seek(to seconds: TimeInterval)
}
