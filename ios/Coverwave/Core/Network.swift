import Foundation
import Network

/// Whether the internet can be reached, and over what, from the system's
/// own path monitor.
@MainActor
@Observable
final class NetworkMonitor {
    static let shared = NetworkMonitor()

    private(set) var isOnline = true
    /// Mobile data or a personal hotspot.
    private(set) var isExpensive = false

    /// Called on the main actor after each change.
    @ObservationIgnored var onChange: [() -> Void] = []

    @ObservationIgnored private let monitor = NWPathMonitor()

    private init() {
        monitor.pathUpdateHandler = { [weak self] path in
            let online = path.status == .satisfied
            let expensive = path.isExpensive
            Task { @MainActor in
                guard let self, self.isOnline != online || self.isExpensive != expensive else { return }
                self.isOnline = online
                self.isExpensive = expensive
                self.onChange.forEach { $0() }
            }
        }
        monitor.start(queue: DispatchQueue(label: "coverwave.network"))
    }
}

extension Error {
    /// The request never reached the server: no network, DNS, timeout.
    var isOffline: Bool {
        guard let e = self as? URLError else { return false }
        switch e.code {
        case .notConnectedToInternet, .networkConnectionLost, .cannotFindHost, .cannotConnectToHost,
            .timedOut, .dnsLookupFailed, .internationalRoamingOff, .dataNotAllowed:
            return true
        default:
            return false
        }
    }
}
