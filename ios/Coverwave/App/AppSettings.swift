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

    /// How touches on the art control playback.
    var gestures: GestureScheme {
        didSet { defaults.set(gestures.rawValue, forKey: "gestures") }
    }

    /// The gesture guide has been shown once.
    var seenGestureGuide: Bool {
        didSet { defaults.set(seenGestureGuide, forKey: "seenGestureGuide") }
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
        seenGestureGuide = defaults.bool(forKey: "seenGestureGuide")
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
