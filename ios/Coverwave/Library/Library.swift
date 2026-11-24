import Foundation

/// A place in a library: `id` is what the provider resolves ("playlists",
/// "album:123"); `title` is the breadcrumb name.
struct LibraryPath: Hashable {
    var id: String
    var title: String

    static let root = LibraryPath(id: "root", title: "library")
}

enum QueuePosition {
    case next
    case last

    var label: String {
        switch self {
        case .next: "Play Next"
        case .last: "Add to Queue"
        }
    }
}

/// One row. Folders open; tracks play (from that track on, through the
/// rest of their album or playlist) and can be queued.
struct LibraryItem: Identifiable {
    let id: String
    var title: String
    var subtitle: String?
    /// Right-aligned: a track count or a duration.
    var detail: String?
    var opens: LibraryPath?
    var play: (@MainActor () async throws -> Void)?
    var enqueue: (@MainActor (QueuePosition) async throws -> Void)?
}

struct QueueSnapshot {
    struct Entry: Identifiable {
        let id: String
        var title: String
        var subtitle: String?
    }

    var current: Entry?
    var upcoming: [Entry]
    /// Why the list may be incomplete.
    var note: String?
}

enum LibraryError: LocalizedError {
    /// The login lacks permission or has expired: log in again.
    case needsLogin
    case message(String)

    var errorDescription: String? {
        switch self {
        case .needsLogin: "Spotify needs you to log in again for your library."
        case .message(let m): m
        }
    }
}

@MainActor
protocol LibraryProvider: AnyObject {
    var name: String { get }
    /// Which queue positions the source supports.
    var queuePositions: [QueuePosition] { get }
    func list(_ path: LibraryPath) async throws -> [LibraryItem]
    func search(_ term: String) async throws -> [LibraryItem]
    func upNext() async throws -> QueueSnapshot
    /// Logs in again with the permissions the library needs.
    func reauthorize()
}

/// Browsing state for the library panel: a stack of places, a short-lived
/// cache so going back does not refetch, a filter, and search.
@MainActor
@Observable
final class LibraryBrowser {
    let provider: any LibraryProvider

    private(set) var stack: [LibraryPath] = [.root]
    private(set) var items: [LibraryItem] = []
    private(set) var loading = false
    private(set) var error: LibraryError?
    /// Results of a library-wide search (root only).
    private(set) var results: [LibraryItem] = []
    private(set) var searching = false
    /// A short confirmation ("queued karma police").
    private(set) var notice: String?

    var query = "" {
        didSet { if query != oldValue { queryChanged() } }
    }

    @ObservationIgnored private var cache: [String: (items: [LibraryItem], at: Date)] = [:]
    @ObservationIgnored private var searchTask: Task<Void, Never>?
    @ObservationIgnored private var noticeTask: Task<Void, Never>?
    private static let ttl: TimeInterval = 300

    init(provider: any LibraryProvider) {
        self.provider = provider
    }

    var path: LibraryPath { stack.last ?? .root }
    var atRoot: Bool { stack.count == 1 }

    /// What to show: the listing, filtered by the query.
    var rows: [LibraryItem] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return items }
        return items.filter {
            $0.title.lowercased().contains(q) || ($0.subtitle?.lowercased().contains(q) ?? false)
        }
    }

    func load(force: Bool = false) async {
        let here = path
        if !force, let hit = cache[here.id], Date().timeIntervalSince(hit.at) < Self.ttl {
            items = hit.items
            error = nil
            return
        }
        loading = true
        error = nil
        defer { if path == here { loading = false } }
        do {
            let fetched = try await provider.list(here)
            cache[here.id] = (fetched, .now)
            if path == here { items = fetched }
        } catch let e as LibraryError {
            if path == here { error = e }
        } catch {
            if path == here { self.error = .message(error.localizedDescription) }
        }
    }

    func open(_ to: LibraryPath) {
        stack.append(to)
        query = ""
        items = []
        Task { await load() }
    }

    func back() {
        guard stack.count > 1 else { return }
        stack.removeLast()
        query = ""
        items = []
        Task { await load() }
    }

    /// Jumps to a breadcrumb.
    func pop(to index: Int) {
        guard index < stack.count - 1 else { return }
        stack.removeLast(stack.count - 1 - index)
        query = ""
        items = []
        Task { await load() }
    }

    func activate(_ item: LibraryItem) async -> Bool {
        if let to = item.opens {
            open(to)
            return false
        }
        guard let play = item.play else { return false }
        do {
            try await play()
            return true
        } catch {
            say((error as? LocalizedError)?.errorDescription ?? error.localizedDescription)
            return false
        }
    }

    func enqueue(_ item: LibraryItem, _ position: QueuePosition) async {
        guard let enqueue = item.enqueue else { return }
        do {
            try await enqueue(position)
            say(position == .next ? "plays next: \(item.title)" : "queued: \(item.title)")
        } catch {
            say((error as? LocalizedError)?.errorDescription ?? error.localizedDescription)
        }
    }

    func reauthorize() {
        cache.removeAll()
        provider.reauthorize()
    }

    private func say(_ text: String) {
        notice = text
        noticeTask?.cancel()
        noticeTask = Task {
            try? await Task.sleep(for: .seconds(2.5))
            if !Task.isCancelled { notice = nil }
        }
    }

    private func queryChanged() {
        searchTask?.cancel()
        let term = query.trimmingCharacters(in: .whitespaces)
        guard atRoot, term.count >= 2 else {
            results = []
            searching = false
            return
        }
        searching = true
        searchTask = Task {
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            let found = (try? await provider.search(term)) ?? []
            guard !Task.isCancelled else { return }
            results = found
            searching = false
        }
    }
}
