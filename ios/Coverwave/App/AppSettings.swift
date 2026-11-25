import Foundation

/// User choices, kept in UserDefaults.
@MainActor
@Observable
final class AppSettings {
    enum Motion: String, CaseIterable, Identifiable {
        /// On, unless Reduce Motion is on.
        case automatic
        case on
        case off

        var id: String { rawValue }

        var label: String {
            switch self {
            case .automatic: "Automatic"
            case .on: "On"
            case .off: "Off"
            }
        }
    }

    @ObservationIgnored private let defaults = UserDefaults.standard

    var motion: Motion {
        didSet { defaults.set(motion.rawValue, forKey: "motion") }
    }

    var tempoLookup: Bool {
        didSet { defaults.set(tempoLookup, forKey: "tempoLookup") }
    }

    var artMode: ArtMode {
        didSet { defaults.set(artMode.key, forKey: "artMode") }
    }

    /// How many characters the art is made of.
    var artDetail: ArtDetail {
        didSet { defaults.set(artDetail.rawValue, forKey: "artDetail") }
    }

    /// Lyrics instead of the cover (the cover keeps moving behind them).
    var showLyrics: Bool {
        didSet { defaults.set(showLyrics, forKey: "showLyrics") }
    }

    var lyricStyle: LyricStyleKind {
        didSet { defaults.set(lyricStyle.key, forKey: "lyricStyle") }
    }

    var lyricScroll: LyricScrollKind {
        didSet { defaults.set(lyricScroll.key, forKey: "lyricScroll") }
    }

    /// Lyric colours: the accent, or the cover's own (letters as windows).
    var lyricPaint: LyricPaintKind {
        didSet { defaults.set(lyricPaint == .cover ? "cover" : "accent", forKey: "lyricPaint") }
    }

    /// How big the large lyric letters are (Detail only makes them finer).
    var lyricSize: LyricSizeKind {
        didSet { defaults.set(lyricSize.key, forKey: "lyricSize") }
    }

    /// How touches on the art control playback.
    var gestures: GestureScheme {
        didSet { defaults.set(gestures.rawValue, forKey: "gestures") }
    }

    /// The gesture guide has been shown once.
    var seenGestureGuide: Bool {
        didSet { defaults.set(seenGestureGuide, forKey: "seenGestureGuide") }
    }

    /// What to save ahead for offline listening.
    var saveAhead: OfflineSaver.Scope {
        didSet { defaults.set(saveAhead.rawValue, forKey: "saveAhead") }
    }

    /// Save ahead over mobile data too (else Wi-Fi only).
    var saveOnMobileData: Bool {
        didSet { defaults.set(saveOnMobileData, forKey: "saveOnMobileData") }
    }

    /// The source to reconnect to on launch.
    var lastSource: SourceKind? {
        didSet { defaults.set(lastSource?.rawValue, forKey: "lastSource") }
    }

    init() {
        // A local copy: reading `self.defaults` in a closure before every
        // stored property is set does not compile.
        let defaults = UserDefaults.standard
        motion = Motion(rawValue: defaults.string(forKey: "motion") ?? "") ?? .automatic
        tempoLookup = defaults.object(forKey: "tempoLookup") as? Bool ?? true
        let savedArtMode = defaults.string(forKey: "artMode")
        artMode = ArtMode.allCases.first { $0.key == savedArtMode } ?? .palette
        lastSource = defaults.string(forKey: "lastSource").flatMap(SourceKind.init(rawValue:))
        artDetail = ArtDetail(rawValue: defaults.string(forKey: "artDetail") ?? "") ?? .fine
        gestures = GestureScheme(rawValue: defaults.string(forKey: "gestures") ?? "") ?? .direct
        showLyrics = defaults.bool(forKey: "showLyrics")
        let savedStyle = defaults.string(forKey: "lyricStyle")
        lyricStyle = LyricStyleKind.allCases.first { $0.key == savedStyle } ?? .bigOnly
        let savedScroll = defaults.string(forKey: "lyricScroll")
        lyricScroll = LyricScrollKind.allCases.first { $0.key == savedScroll } ?? .up
        lyricPaint = defaults.string(forKey: "lyricPaint") == "cover" ? .cover : .accent
        let savedSize = defaults.string(forKey: "lyricSize")
        lyricSize = LyricSizeKind.allCases.first { $0.key == savedSize } ?? .medium
        seenGestureGuide = defaults.bool(forKey: "seenGestureGuide")
        saveAhead = OfflineSaver.Scope(rawValue: defaults.string(forKey: "saveAhead") ?? "") ?? .liked
        saveOnMobileData = defaults.bool(forKey: "saveOnMobileData")
    }

    func animates(reduceMotion: Bool) -> Bool {
        switch motion {
        case .automatic: !reduceMotion
        case .on: true
        case .off: false
        }
    }
}

extension ArtMode: CaseIterable, Identifiable {
    public static var allCases: [ArtMode] { [.palette, .trueColor, .duotone] }
    public var id: String { key }

    var key: String {
        switch self {
        case .palette: "palette"
        case .trueColor: "truecolor"
        case .duotone: "duotone"
        }
    }

    var label: String {
        switch self {
        case .palette: "Palette"
        case .trueColor: "True colour"
        case .duotone: "Duotone"
        }
    }
}

extension LyricStyleKind: CaseIterable, Identifiable {
    public static var allCases: [LyricStyleKind] { [.bigOnly, .bigCurrent, .text] }
    public var id: String { key }

    var key: String {
        switch self {
        case .bigCurrent: "big"
        case .text: "text"
        case .bigOnly: "big_only"
        }
    }

    var label: String {
        switch self {
        case .bigCurrent: "Big Line"
        case .text: "Text"
        case .bigOnly: "Big Only"
        }
    }
}

extension LyricScrollKind: CaseIterable, Identifiable {
    public static var allCases: [LyricScrollKind] { [.up, .down] }
    public var id: String { key }

    var key: String {
        switch self {
        case .up: "up"
        case .down: "down"
        }
    }

    var label: String {
        switch self {
        case .up: "Up"
        case .down: "Down"
        }
    }
}

extension LyricSizeKind: CaseIterable, Identifiable {
    public static var allCases: [LyricSizeKind] { [.small, .medium, .large] }
    public var id: String { key }

    var key: String {
        switch self {
        case .small: "small"
        case .medium: "medium"
        case .large: "large"
        }
    }

    var label: String {
        switch self {
        case .small: "Small"
        case .medium: "Medium"
        case .large: "Large"
        }
    }
}
