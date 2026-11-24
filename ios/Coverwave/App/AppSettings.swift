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

    var listenWithMicrophone: Bool {
        didSet { defaults.set(listenWithMicrophone, forKey: "listenWithMicrophone") }
    }

    var tempoLookup: Bool {
        didSet { defaults.set(tempoLookup, forKey: "tempoLookup") }
    }

    var artMode: ArtMode {
        didSet { defaults.set(artMode.key, forKey: "artMode") }
    }

    /// The source to reconnect to on launch.
    var lastSource: SourceKind? {
        didSet { defaults.set(lastSource?.rawValue, forKey: "lastSource") }
    }

    init() {
        motion = Motion(rawValue: defaults.string(forKey: "motion") ?? "") ?? .automatic
        listenWithMicrophone = defaults.bool(forKey: "listenWithMicrophone")
        tempoLookup = defaults.object(forKey: "tempoLookup") as? Bool ?? true
        artMode = ArtMode.allCases.first { $0.key == defaults.string(forKey: "artMode") } ?? .palette
        lastSource = defaults.string(forKey: "lastSource").flatMap(SourceKind.init(rawValue:))
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
