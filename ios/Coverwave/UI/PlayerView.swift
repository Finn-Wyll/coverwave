import SwiftUI

/// The one screen: the cover, and nothing else until you touch it. The art
/// is the control (see `GestureScheme`); text appears in the cover's own
/// terminal style while you touch, and the library and queue slide over it
/// following the finger.
struct PlayerView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var showSettings = false
    @State private var hudVisible = true
    @State private var status: String?
    @State private var hideTask: Task<Void, Never>?
    @State private var statusTask: Task<Void, Never>?

    /// Live gesture state.
    @State private var axis: Axis?
    @State private var artOffset: CGFloat = 0
    @State private var scrubStart: (fraction: Double, x: CGFloat)?
    @State private var scrub: Double?
    @State private var dialCenter: CGPoint?
    @State private var dialPick: DialOption?

    /// Panels: open state plus a live drag offset from their resting place.
    @State private var libraryOpen = false
    @State private var libraryDrag: CGFloat = 0
    @State private var queueOpen = false
    @State private var queueDrag: CGFloat = 0

    var body: some View {
        GeometryReader { geo in
            let size = geo.size
            ZStack {
                Theme.page.ignoresSafeArea()
                art(size: size)
                if model.isFollowing {
                    following(size: size)
                } else {
                    ConnectScreen()
                }
            }
        }
        .tint(model.accent)
        .preferredColorScheme(.dark)
        .sheet(isPresented: $showSettings) {
            SettingsView()
                .presentationDetents([.large])
                .presentationBackground(Theme.page)
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { model.sceneBecameActive() }
        }
        .onChange(of: model.nowPlaying?.trackID) { poke() }
        .onChange(of: model.nowPlaying?.isPlaying) { poke() }
        .statusBarHidden(!hudVisible || libraryOpen)
        .persistentSystemOverlays(hudVisible ? .automatic : .hidden)
    }

    // MARK: Layers

    private func art(size: CGSize) -> some View {
        let animate = model.settings.animates(reduceMotion: reduceMotion)
        return CoverCanvas(animate: animate)
            .id(model.coverHash)
            .transition(.opacity)
            .ignoresSafeArea(edges: .bottom)
            .offset(x: reduceMotion ? 0 : artOffset)
            // Dim behind the scrub time, the dial and open panels.
            .overlay {
                Theme.page
                    .opacity(scrub != nil || dialCenter != nil ? 0.45 : panelDim(size: size))
                    .ignoresSafeArea()
                    .allowsHitTesting(false)
            }
            .animation(.easeInOut(duration: 0.45), value: model.coverHash)
    }

    @ViewBuilder private func following(size: CGSize) -> some View {
        GestureSurface { handle($0, size: size) }
            .ignoresSafeArea()
            .accessibilityElement()
            .accessibilityLabel(accessibilityTitle)
            .accessibilityHint("Double-tap to play or pause. More actions are available.")
            .accessibilityAction { playPause() }
            .accessibilityAction(named: "Next") { next() }
            .accessibilityAction(named: "Previous") { previous() }
            .accessibilityAction(named: "Library") { setLibrary(open: true, velocity: 0, size: size) }
            .accessibilityAction(named: "Queue") { setQueue(open: true, velocity: 0, size: size) }
            .accessibilityAction(named: "Settings") { showSettings = true }

        if let center = dialCenter {
            DialView(selected: dialPick)
                .position(center)
                .transition(.opacity)
                .allowsHitTesting(false)
        }

        VStack {
            Spacer()
            HUD(
                track: model.nowPlaying,
                sourceName: model.activeKind?.name ?? "",
                elsewhere: model.activeKind == .spotify
                    ? model.spotify.device.flatMap { $0.isPhone ? nil : $0.name } : nil,
                status: status ?? modelStatus,
                scrub: scrub,
                onScrub: barScrub
            )
            .opacity(hudVisible || scrub != nil ? 1 : 0)
            .allowsHitTesting(hudVisible)
            .accessibilityHidden(!hudVisible)
        }
        .animation(.easeInOut(duration: 0.25), value: hudVisible)

        // Tapping the art around an open panel closes it.
        if libraryOpen || queueOpen {
            Color.clear
                .contentShape(Rectangle())
                .ignoresSafeArea()
                .onTapGesture {
                    setLibrary(open: false, velocity: 0, size: size)
                    setQueue(open: false, velocity: 0, size: size)
                }
                .accessibilityHidden(true)
        }

        queuePanel(size: size)
        libraryPanel(size: size)

        if !model.settings.seenGestureGuide {
            GestureGuide(scheme: model.settings.gestures) {
                withAnimation(.easeOut(duration: 0.25)) { model.settings.seenGestureGuide = true }
            }
            .transition(.opacity)
        }
    }

    private func libraryHeight(_ size: CGSize) -> CGFloat { size.height * 0.86 }
    private func queueHeight(_ size: CGSize) -> CGFloat { size.height * 0.58 }

    @ViewBuilder private func libraryPanel(size: CGSize) -> some View {
        let h = libraryHeight(size)
        let rest: CGFloat = libraryOpen ? 0 : h + 40
        let offset = libraryOffset(rest + libraryDrag, height: h)
        if let browser = model.library, libraryOpen || libraryDrag != 0 {
            LibraryPanel(
                browser: browser,
                onHeaderDrag: { dy, velocity in
                    if let velocity {
                        let close = projected(dy, velocity: velocity) > h * 0.3
                        setLibrary(open: !close, velocity: velocity, size: size)
                    } else {
                        libraryDrag = dy
                    }
                },
                close: { setLibrary(open: false, velocity: 0, size: size) }
            )
            .frame(height: h)
            .frame(maxHeight: .infinity, alignment: .bottom)
            .ignoresSafeArea(edges: .bottom)
            .offset(y: offset)
            .accessibilityAddTraits(.isModal)
        }
    }

    @ViewBuilder private func queuePanel(size: CGSize) -> some View {
        let h = queueHeight(size)
        let rest: CGFloat = queueOpen ? 0 : -(h + 40)
        let offset = queueOffset(rest + queueDrag, height: h)
        if queueOpen || queueDrag != 0 {
            QueuePanel(
                provider: model.library?.provider,
                spotify: model.activeKind == .spotify ? model.spotify : nil,
                onHeaderDrag: { dy, velocity in
                    if let velocity {
                        let close = projected(dy, velocity: velocity) < -h * 0.3
                        setQueue(open: !close, velocity: velocity, size: size)
                    } else {
                        queueDrag = dy
                    }
                },
                close: { setQueue(open: false, velocity: 0, size: size) }
            )
            .id(model.nowPlaying?.trackID)
            .frame(height: h)
            .frame(maxHeight: .infinity, alignment: .top)
            .ignoresSafeArea(edges: .top)
            .offset(y: offset)
            .accessibilityAddTraits(.isModal)
        }
    }

    /// Past fully open, the panel resists.
    private func libraryOffset(_ y: CGFloat, height: CGFloat) -> CGFloat {
        y < 0 ? rubberband(y, limit: height) : y
    }

    private func queueOffset(_ y: CGFloat, height: CGFloat) -> CGFloat {
        y > 0 ? rubberband(y, limit: height) : y
    }

    /// The art dims as a panel comes in, in step with the finger.
    private func panelDim(size: CGSize) -> Double {
        let lib = libraryOpen ? 1 : min(1, max(0, -libraryDrag / libraryHeight(size)))
        let q = queueOpen ? 1 : min(1, max(0, queueDrag / queueHeight(size)))
        return 0.5 * max(lib, q)
    }

    // MARK: Gestures

    private func handle(_ event: SurfaceEvent, size: CGSize) {
        poke()
        let scheme = model.settings.gestures
        switch event {
        case .tap(let p):
            if !model.settings.seenGestureGuide { return }
            switch scheme {
            case .direct, .dial: playPause()
            case .zones:
                let third = p.x / max(1, size.width)
                if third < 1 / 3 { previous() } else if third > 2 / 3 { next() } else { playPause() }
            }

        case .twoFingerTap:
            showSettings = true

        case .twoFingerSwipe:
            model.settings.showLyrics.toggle()
            Haptics.commit()
            if !model.settings.showLyrics {
                flash("cover")
            } else {
                switch model.lyricsStatus {
                case .found?: flash("lyrics")
                case .searching?: flash("looking up lyrics")
                case .instrumental?: flash("instrumental, no lyrics")
                default: flash("no lyrics found for this song")
                }
            }

        case .panChanged(let t):
            if axis == nil { axis = abs(t.width) > abs(t.height) ? .horizontal : .vertical }
            if axis == .horizontal {
                artOffset = rubberband(t.width, limit: 60)
                let p = projected(t.width, velocity: 0)
                flash(p < -swipeCommit ? "> next" : p > swipeCommit ? "< previous" : nil, hold: true)
            } else {
                // The panel follows the finger 1:1 from its edge.
                if t.height < 0 {
                    libraryDrag = t.height
                    queueDrag = 0
                } else {
                    queueDrag = t.height
                    libraryDrag = 0
                }
            }

        case .panEnded(let t, let v):
            defer { axis = nil }
            if axis == .horizontal {
                let p = projected(t.width, velocity: v.width)
                if p < -swipeCommit { next() } else if p > swipeCommit { previous() } else { flash(nil) }
                withAnimation(spring(velocity: v.width, distance: -artOffset)) { artOffset = 0 }
            } else if axis == .vertical {
                let p = projected(t.height, velocity: v.height)
                if t.height < 0 {
                    setLibrary(open: p < -swipeCommit * 1.4, velocity: v.height, size: size)
                } else {
                    setQueue(open: p > swipeCommit * 1.4, velocity: v.height, size: size)
                }
            }

        case .holdBegan(let p):
            Haptics.tick()
            if scheme == .dial {
                withAnimation(.easeOut(duration: 0.15)) { dialCenter = p }
                dialPick = nil
            } else if let np = model.nowPlaying, let d = np.duration, d > 0 {
                let f = min(1, np.position(at: .now) / d)
                scrubStart = (f, p.x)
                scrub = f
            }

        case .holdMoved(let p):
            if let c = dialCenter {
                let pick = DialOption.at(CGSize(width: p.x - c.x, height: p.y - c.y))
                if pick != dialPick {
                    dialPick = pick
                    if pick != nil { Haptics.tick() }
                }
            } else if let start = scrubStart {
                // A full screen width spans the whole track.
                scrub = min(1, max(0, start.fraction + Double((p.x - start.x) / max(1, size.width))))
            }

        case .holdEnded:
            if dialCenter != nil {
                let pick = dialPick
                withAnimation(.easeOut(duration: 0.15)) { dialCenter = nil }
                dialPick = nil
                switch pick {
                case .library: setLibrary(open: true, velocity: 0, size: size)
                case .queue: setQueue(open: true, velocity: 0, size: size)
                case .previous: previous()
                case .next: next()
                case nil: break
                }
            } else if let f = scrub {
                finishScrub(f)
            }
            scrubStart = nil
        }
    }

    /// The HUD's bar can be dragged in every scheme.
    private func barScrub(_ fraction: Double?, _ done: Bool) {
        poke()
        guard let fraction else { return }
        if done { finishScrub(fraction) } else { scrub = fraction }
    }

    private func finishScrub(_ fraction: Double) {
        if let np = model.nowPlaying, let d = np.duration, d > 0 {
            model.active?.seek(to: fraction * d)
            Haptics.commit()
            flash("> \(timeText(fraction * d))")
        }
        // Hold the new position until the source reports it.
        Task {
            try? await Task.sleep(for: .milliseconds(600))
            scrub = nil
        }
    }

    private var swipeCommit: CGFloat { 90 }

    /// A spring that continues at the finger's release velocity.
    private func spring(velocity: CGFloat, distance: CGFloat) -> Animation {
        if reduceMotion { return .easeInOut(duration: 0.2) }
        let relative = abs(distance) > 1 ? velocity / distance : 0
        return .interpolatingSpring(duration: 0.35, bounce: 0, initialVelocity: relative)
    }

    private func setLibrary(open: Bool, velocity: CGFloat, size: CGSize) {
        if open { setQueue(open: false, velocity: 0, size: size) }
        if open != libraryOpen { Haptics.commit() }
        let h = libraryHeight(size)
        let from = (libraryOpen ? 0 : h + 40) + libraryDrag
        let to: CGFloat = open ? 0 : h + 40
        withAnimation(spring(velocity: velocity, distance: to - from)) {
            libraryOpen = open
            libraryDrag = 0
        }
    }

    private func setQueue(open: Bool, velocity: CGFloat, size: CGSize) {
        if open { setLibrary(open: false, velocity: 0, size: size) }
        if open != queueOpen { Haptics.commit() }
        let h = queueHeight(size)
        let from = (queueOpen ? 0 : -(h + 40)) + queueDrag
        let to: CGFloat = open ? 0 : -(h + 40)
        withAnimation(spring(velocity: velocity, distance: to - from)) {
            queueOpen = open
            queueDrag = 0
        }
    }

    // MARK: Actions

    private func playPause() {
        guard let source = model.active else { return }
        let playing = model.nowPlaying?.isPlaying ?? false
        source.playPause()
        Haptics.commit()
        flash(playing ? "|| paused" : "> playing")
    }

    private func next() {
        model.active?.next()
        Haptics.commit()
        flash(">> next")
    }

    private func previous() {
        model.active?.previous()
        Haptics.commit()
        flash("<< previous")
    }

    /// Shows the HUD, then fades it while music plays.
    private func poke() {
        if !hudVisible { withAnimation(.easeOut(duration: 0.2)) { hudVisible = true } }
        hideTask?.cancel()
        hideTask = Task {
            try? await Task.sleep(for: .seconds(3.5))
            guard !Task.isCancelled else { return }
            // Paused, or nothing playing: keep it up.
            if model.nowPlaying?.isPlaying == true, scrub == nil, dialCenter == nil {
                withAnimation(.easeInOut(duration: 0.6)) { hudVisible = false }
            }
        }
    }

    private func flash(_ text: String?, hold: Bool = false) {
        statusTask?.cancel()
        withAnimation(.easeOut(duration: 0.15)) { status = text }
        guard text != nil, !hold else { return }
        statusTask = Task {
            try? await Task.sleep(for: .seconds(1.6))
            guard !Task.isCancelled else { return }
            withAnimation(.easeOut(duration: 0.3)) { status = nil }
        }
    }

    private var modelStatus: String? {
        switch model.core.modelState {
        case .downloading(let f): "preparing covers \(Int(f * 100))%"
        case .loading: "preparing covers"
        default: model.nowPlaying?.isPlaying == false ? "|| paused" : nil
        }
    }

    private var accessibilityTitle: String {
        guard let np = model.nowPlaying else { return "Cover. Nothing playing" }
        return "\(np.title) by \(np.artist), \(np.isPlaying ? "playing" : "paused")"
    }
}

/// Before a source is connected: pick one, in the same terminal style.
private struct ConnectScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Spacer()
            VStack(alignment: .leading, spacing: 4) {
                Text("coverwave")
                    .font(Term.font(22, .semibold))
                    .foregroundStyle(Term.ink)
                Text("follow your music. the cover moves with it.")
                    .font(Term.font(13))
                    .foregroundStyle(Term.dim)
            }
            VStack(alignment: .leading, spacing: 0) {
                ForEach(SourceKind.allCases) { kind in
                    let source = model.source(kind)
                    let busy = model.activeKind == kind && source.status == .connecting
                    let disabled = kind != .appleMusic && !model.spotify.isConfigured
                    Button {
                        model.choose(kind)
                    } label: {
                        HStack(alignment: .firstTextBaseline) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("> \(kind.name.lowercased())")
                                    .font(Term.font(17, .medium))
                                    .foregroundStyle(disabled ? Term.faint : Term.ink)
                                Text("  " + kind.blurb)
                                    .font(Term.font(12))
                                    .foregroundStyle(Term.dim)
                            }
                            Spacer()
                            if busy {
                                Text(kind == .offline ? "waiting for spotify..." : "connecting...")
                                    .font(Term.font(12)).foregroundStyle(.tint)
                            }
                        }
                        .frame(minHeight: 52)
                    }
                    .buttonStyle(TermRowStyle())
                    .disabled(busy || disabled)
                }
            }
            if let kind = model.activeKind, model.source(kind).status != .connecting,
                let message = model.source(kind).status.message
            {
                Text("! " + message.lowercased())
                    .font(Term.font(12))
                    .foregroundStyle(Term.dim)
            }
        }
        .padding(.horizontal, 24)
        .padding(.bottom, 32)
        .frame(maxWidth: 560, alignment: .leading)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(alignment: .bottom) {
            LinearGradient(colors: [Theme.page.opacity(0), Theme.page.opacity(0.9)], startPoint: .top, endPoint: .center)
                .ignoresSafeArea()
        }
    }
}
