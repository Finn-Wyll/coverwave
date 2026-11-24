import SwiftUI

/// The library, as a terminal listing: a path, a `/` search line, `..` to go
/// up, folders ending in `/`. It rises from the bottom (where the swipe up
/// came from) and goes back down the same way.
struct LibraryPanel: View {
    @Bindable var browser: LibraryBrowser
    /// Drags on the header move the panel; the owner decides open/closed.
    let onHeaderDrag: (CGFloat, CGFloat?) -> Void
    let close: () -> Void
    @FocusState private var searchFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider().overlay(Term.faint)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if !browser.atRoot {
                        row(title: "..", subtitle: nil, detail: nil, folder: false) { browser.back() }
                    }
                    content
                }
                .padding(.vertical, 6)
            }
            .scrollDismissesKeyboard(.immediately)
            if let notice = browser.notice {
                Text(notice)
                    .font(Term.font(13, .medium))
                    .foregroundStyle(.tint)
                    .lineLimit(1)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.page)
                    .transition(.opacity)
            }
        }
        .animation(.easeOut(duration: 0.2), value: browser.notice)
        .background(Theme.page.opacity(0.97))
        .task(id: browser.path) { await browser.load() }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            Capsule()
                .fill(Term.faint)
                .frame(width: 36, height: 4)
                .frame(maxWidth: .infinity)
                .accessibilityHidden(true)
            HStack(alignment: .firstTextBaseline) {
                breadcrumb
                Spacer(minLength: 12)
                Button("esc", action: close)
                    .font(Term.font(13))
                    .foregroundStyle(Term.dim)
                    .accessibilityLabel("Close library")
            }
            HStack(spacing: 8) {
                Text("/").foregroundStyle(.tint)
                TextField(
                    "", text: $browser.query,
                    prompt: Text(browser.atRoot ? "search \(browser.provider.name)" : "filter")
                        .foregroundColor(Term.faint)
                )
                .focused($searchFocused)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.search)
                .foregroundStyle(Term.ink)
            }
            .font(Term.font(15))
        }
        .padding(.horizontal, 20)
        .padding(.top, 10)
        .padding(.bottom, 12)
        .contentShape(Rectangle())
        .gesture(
            DragGesture(minimumDistance: 8)
                .onChanged { onHeaderDrag($0.translation.height, nil) }
                .onEnded { onHeaderDrag($0.translation.height, $0.velocity.height) })
    }

    /// `~/spotify/playlists/kid a`, each step tappable.
    private var breadcrumb: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 0) {
                Text("~/\(browser.provider.name)")
                    .foregroundStyle(Term.dim)
                    .onTapGesture { browser.pop(to: 0) }
                ForEach(Array(browser.stack.enumerated()).dropFirst(), id: \.offset) { i, step in
                    Text("/\(step.title.lowercased())")
                        .foregroundStyle(i == browser.stack.count - 1 ? Term.ink : Term.dim)
                        .onTapGesture { browser.pop(to: i) }
                }
            }
            .font(Term.font(14, .medium))
            .lineLimit(1)
        }
        .defaultScrollAnchor(.trailing)
    }

    @ViewBuilder private var content: some View {
        if let error = browser.error {
            VStack(alignment: .leading, spacing: 8) {
                Text("! \(error.errorDescription ?? "Something went wrong.")")
                    .font(Term.font(14))
                    .foregroundStyle(Term.ink)
                    .lineLimit(4)
                if case .needsLogin = error {
                    textButton("> log in to \(browser.provider.name) again") { browser.reauthorize() }
                }
                textButton("> try again") { Task { await browser.load(force: true) } }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
        } else if browser.loading && browser.items.isEmpty {
            // A skeleton in glyphs: rows shaped like the listing.
            ForEach(0..<6, id: \.self) { i in
                Text(String(repeating: ".", count: 14 + (i * 7) % 12))
                    .font(Term.font(15))
                    .foregroundStyle(Term.faint)
                    .padding(.horizontal, 20)
                    .frame(height: 48, alignment: .leading)
            }
            .accessibilityLabel("Loading")
        } else {
            let rows = browser.rows
            ForEach(rows) { item in itemRow(item) }
            if browser.atRoot && !browser.query.isEmpty {
                Text(browser.searching ? "searching..." : browser.results.isEmpty ? "no matches" : "matches")
                    .font(Term.font(12))
                    .foregroundStyle(Term.dim)
                    .padding(.horizontal, 20)
                    .padding(.top, 14)
                    .padding(.bottom, 4)
                ForEach(browser.results) { item in itemRow(item) }
            } else if rows.isEmpty && !browser.loading {
                Text(browser.query.isEmpty ? "empty" : "no matches for \"\(browser.query)\"")
                    .font(Term.font(14))
                    .foregroundStyle(Term.dim)
                    .padding(20)
            }
        }
    }

    private func itemRow(_ item: LibraryItem) -> some View {
        let positions = item.enqueue == nil ? [] : browser.provider.queuePositions
        return row(title: item.title, subtitle: item.subtitle, detail: item.detail, folder: item.opens != nil) {
            Task {
                if await browser.activate(item) { close() }
            }
        }
        .contextMenu {
            ForEach(positions, id: \.label) { position in
                Button(position.label) { Task { await browser.enqueue(item, position) } }
            }
        }
        .accessibilityActions {
            ForEach(positions, id: \.label) { position in
                Button(position.label) { Task { await browser.enqueue(item, position) } }
            }
        }
    }

    private func row(
        title: String, subtitle: String?, detail: String?, folder: Bool, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(folder ? title.lowercased() + "/" : title.lowercased())
                        .font(Term.font(15, folder ? .medium : .regular))
                        .foregroundStyle(Term.ink)
                    if let subtitle {
                        Text(subtitle.lowercased())
                            .font(Term.font(12))
                            .foregroundStyle(Term.dim)
                    }
                }
                .lineLimit(1)
                Spacer(minLength: 8)
                if let detail {
                    Text(detail)
                        .font(Term.font(12))
                        .foregroundStyle(Term.dim)
                }
            }
            .padding(.horizontal, 20)
            .frame(minHeight: 48)
        }
        .buttonStyle(TermRowStyle())
    }

    private func textButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title).font(Term.font(14, .medium)).foregroundStyle(.tint)
                .frame(minHeight: 36, alignment: .leading)
        }
        .buttonStyle(TermRowStyle())
    }
}

/// What plays next. Drops from the top (where the swipe down came from).
struct QueuePanel: View {
    let provider: (any LibraryProvider)?
    let onHeaderDrag: (CGFloat, CGFloat?) -> Void
    let close: () -> Void
    @State private var snapshot: QueueSnapshot?
    @State private var error: LibraryError?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // The list only scrolls; it never moves the panel.
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    content
                }
                .padding(.vertical, 6)
                .padding(.bottom, 12)
            }
            .scrollBounceBehavior(.basedOnSize)
            // A scroll-edge fade where the list meets the grabber, instead
            // of a hard rule.
            .mask {
                VStack(spacing: 0) {
                    Color.black
                    LinearGradient(colors: [.black, .clear], startPoint: .top, endPoint: .bottom)
                        .frame(height: 24)
                }
            }
            grabber
        }
        .background(Theme.page.opacity(0.97))
        .task { await refresh() }
    }

    /// The only part that moves the panel: it follows the finger, and a
    /// flick up (projected) throws it shut. Vertical drags only, after a
    /// little hysteresis, so a sideways brush does nothing.
    private var grabber: some View {
        VStack(spacing: 8) {
            HStack {
                Text("~/queue").font(Term.font(14, .medium)).foregroundStyle(Term.ink)
                Spacer()
                Button("esc", action: close)
                    .font(Term.font(13))
                    .foregroundStyle(Term.dim)
                    .accessibilityLabel("Close queue")
            }
            Capsule()
                .fill(Term.faint)
                .frame(width: 36, height: 4)
                .accessibilityHidden(true)
        }
        .padding(.horizontal, 20)
        .padding(.top, 10)
        .padding(.bottom, 12)
        .contentShape(Rectangle())
        .gesture(
            DragGesture(minimumDistance: 10)
                .onChanged { v in
                    guard abs(v.translation.height) > abs(v.translation.width) else { return }
                    onHeaderDrag(v.translation.height, nil)
                }
                .onEnded { v in onHeaderDrag(v.translation.height, v.velocity.height) })
    }

    @ViewBuilder private var content: some View {
        if let error {
            Text("! \(error.errorDescription ?? "")")
                .font(Term.font(14)).foregroundStyle(Term.ink)
                .padding(20)
            if case .needsLogin = error, let provider {
                Button {
                    provider.reauthorize()
                } label: {
                    Text("> log in to \(provider.name) again").font(Term.font(14, .medium)).foregroundStyle(.tint)
                }
                .padding(.horizontal, 20)
            }
        } else if let snapshot {
            if let current = snapshot.current {
                line(prefix: "now ", current, highlight: true)
            }
            if snapshot.upcoming.isEmpty {
                Text("nothing queued. long-press a song in the library to add it.")
                    .font(Term.font(13)).foregroundStyle(Term.dim)
                    .padding(.horizontal, 20).padding(.vertical, 12)
            }
            ForEach(Array(snapshot.upcoming.enumerated()), id: \.element.id) { i, entry in
                line(prefix: String(format: "%2d. ", i + 1), entry, highlight: false)
            }
            if let note = snapshot.note {
                Text(note).font(Term.font(12)).foregroundStyle(Term.dim)
                    .padding(.horizontal, 20).padding(.vertical, 12)
            }
        } else {
            Text("reading queue...").font(Term.font(14)).foregroundStyle(Term.faint).padding(20)
        }
    }

    private func line(prefix: String, _ entry: QueueSnapshot.Entry, highlight: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 0) {
            Text(prefix)
                .foregroundStyle(highlight ? AnyShapeStyle(.tint) : AnyShapeStyle(Term.dim))
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.title.lowercased()).foregroundStyle(Term.ink)
                if let s = entry.subtitle {
                    Text(s.lowercased()).font(Term.font(12)).foregroundStyle(Term.dim)
                }
            }
            .lineLimit(1)
        }
        .font(Term.font(15))
        .padding(.horizontal, 20)
        .frame(minHeight: 46, alignment: .leading)
        .accessibilityElement(children: .combine)
    }

    private func refresh() async {
        guard let provider else { return }
        do {
            snapshot = try await provider.upNext()
            error = nil
        } catch let e as LibraryError {
            error = e
        } catch {
            self.error = .message(error.localizedDescription)
        }
    }
}
