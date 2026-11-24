import SwiftUI

/// The one screen: the cover fills it, and the controls float over it on
/// translucent panels. Tap the art (or flick the panel down) to hide the
/// chrome; tap again to bring it back.
struct PlayerView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var chromeHidden = false
    @State private var showSettings = false

    var body: some View {
        let animate = model.settings.animates(reduceMotion: reduceMotion)
        ZStack {
            Theme.page.ignoresSafeArea()
            CoverCanvas(animate: animate)
                .id(model.coverHash)
                .transition(.opacity)
                .ignoresSafeArea(edges: .bottom)
                .contentShape(Rectangle())
                .onTapGesture { setChrome(hidden: !chromeHidden) }
                .accessibilityAction(named: chromeHidden ? "Show controls" : "Hide controls") {
                    setChrome(hidden: !chromeHidden)
                }
        }
        .animation(.easeInOut(duration: 0.45), value: model.coverHash)
        .overlay(alignment: .top) {
            TopBar(showSettings: $showSettings)
                .opacity(chromeHidden ? 0 : 1)
                .offset(y: chromeHidden && !reduceMotion ? -16 : 0)
                .allowsHitTesting(!chromeHidden)
        }
        .overlay(alignment: .bottom) {
            Group {
                if model.isFollowing {
                    NowPlayingPanel(hidden: $chromeHidden)
                } else {
                    ConnectCard()
                        .opacity(chromeHidden ? 0 : 1)
                        .allowsHitTesting(!chromeHidden)
                }
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 4)
            .frame(maxWidth: 560)
        }
        .tint(model.accent)
        .sheet(isPresented: $showSettings) {
            SettingsView()
                .presentationDetents([.medium, .large])
                .presentationBackground(.regularMaterial)
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { model.sceneBecameActive() }
        }
        .statusBarHidden(chromeHidden)
        .persistentSystemOverlays(chromeHidden ? .hidden : .automatic)
    }

    private func setChrome(hidden: Bool) {
        withAnimation(reduceMotion ? .easeInOut(duration: 0.2) : .spring(response: 0.4, dampingFraction: 1)) {
            chromeHidden = hidden
        }
    }
}

/// Source picker (anchored to its chip) and settings.
private struct TopBar: View {
    @Environment(AppModel.self) private var model
    @Binding var showSettings: Bool

    var body: some View {
        HStack(spacing: 10) {
            if let kind = model.activeKind {
                Menu {
                    ForEach(SourceKind.allCases) { option in
                        Button {
                            model.choose(option)
                        } label: {
                            Label(option.name, systemImage: option.symbol)
                        }
                        .disabled(option == .spotify && !model.spotify.isConfigured)
                    }
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: kind.symbol)
                        Text(kind.name)
                        Image(systemName: "chevron.down").font(.caption2.weight(.bold))
                    }
                    .font(.subheadline.weight(.medium))
                    .padding(.horizontal, 14)
                    .frame(height: 36)
                    .background(Panel(cornerRadius: 18))
                }
                .accessibilityLabel("Source: \(kind.name)")
            }
            Spacer()
            ModelBadge()
            Button {
                showSettings = true
            } label: {
                Image(systemName: "slider.horizontal.3")
                    .font(.subheadline.weight(.medium))
                    .frame(width: 36, height: 36)
                    .background(Panel(cornerRadius: 18))
            }
            .buttonStyle(PressStyle(scale: 0.9))
            .accessibilityLabel("Settings")
        }
        .foregroundStyle(.primary)
        .padding(.horizontal, 16)
        .padding(.top, 6)
    }
}

/// Shown only while the cover-analysis model downloads or loads.
private struct ModelBadge: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let text: String? =
            switch model.core.modelState {
            case .downloading(let f): "Preparing covers \(Int(f * 100))%"
            case .loading: "Preparing covers"
            default: nil
            }
        if let text {
            Text(text)
                .font(.caption.weight(.medium).monospacedDigit())
                .foregroundStyle(.secondary)
                .padding(.horizontal, 12)
                .frame(height: 28)
                .background(Panel(cornerRadius: 14))
                .transition(.opacity)
        }
    }
}

/// Track, progress and transport. Drag it down to hide; it follows the
/// finger, resists being pulled up, and is thrown by the release velocity.
private struct NowPlayingPanel: View {
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Binding var hidden: Bool
    @State private var drag: CGFloat = 0
    @State private var height: CGFloat = 240

    private var hiddenOffset: CGFloat { height + 60 }

    var body: some View {
        content
            .padding(.horizontal, 20)
            .padding(.top, 10)
            .padding(.bottom, 16)
            .background(Panel())
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
            .offset(y: hidden ? hiddenOffset : resisted(drag))
            .opacity(reduceMotion && hidden ? 0 : 1)
            .gesture(
                DragGesture(minimumDistance: 10)
                    .onChanged { drag = $0.translation.height }
                    .onEnded(release)
            )
            .accessibilityAction(named: "Hide controls") { settle(hide: true, velocity: 0) }
    }

    @ViewBuilder private var content: some View {
        VStack(alignment: .leading, spacing: 14) {
            Capsule()
                .fill(.secondary.opacity(0.6))
                .frame(width: 36, height: 5)
                .frame(maxWidth: .infinity)
                .accessibilityHidden(true)
            if let source = model.active, let np = model.nowPlaying {
                VStack(alignment: .leading, spacing: 2) {
                    Text(np.title)
                        .font(.title3.weight(.semibold))
                        .tracking(-0.2)
                    Text(np.artist)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                .lineLimit(1)
                .accessibilityElement(children: .combine)
                Scrubber(track: np) { source.seek(to: $0) }
                TransportControls(source: source, isPlaying: np.isPlaying, enabled: true)
            } else if let source = model.active {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Nothing playing").font(.title3.weight(.semibold))
                    Text("Start something in \(source.kind.name).")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                TransportControls(source: source, isPlaying: false, enabled: false)
            }
        }
    }

    /// Past the resting position upward, the panel follows less and less.
    private func resisted(_ y: CGFloat) -> CGFloat {
        guard y < 0 else { return y }
        let d = max(height, 1)
        let c: CGFloat = 0.55
        return -(-y * d * c) / (d + c * -y)
    }

    private func release(_ value: DragGesture.Value) {
        // Decide from where the flick is heading, not where it let go.
        let hide = value.predictedEndTranslation.height > height * 0.5
        settle(hide: hide, velocity: value.velocity.height)
    }

    private func settle(hide: Bool, velocity: CGFloat) {
        let target = hide ? hiddenOffset : 0
        let distance = target - resisted(drag)
        let relative = abs(distance) > 1 ? velocity / distance : 0
        let animation: Animation =
            reduceMotion
            ? .easeInOut(duration: 0.2)
            : .interpolatingSpring(duration: 0.35, bounce: hide ? 0 : 0.15, initialVelocity: relative)
        withAnimation(animation) {
            hidden = hide
            drag = 0
        }
    }
}

/// Before a source is connected: pick one.
private struct ConnectCard: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Follow your music")
                    .font(.title2.weight(.semibold))
                    .tracking(-0.3)
                Text("Covers from Spotify or Apple Music, in motion.")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            ForEach(SourceKind.allCases) { kind in
                let source = model.source(kind)
                let busy = model.activeKind == kind && source.status == .connecting
                Button {
                    model.choose(kind)
                } label: {
                    HStack(spacing: 12) {
                        Image(systemName: kind.symbol).frame(width: 24)
                        Text(kind.name).font(.body.weight(.semibold))
                        Spacer()
                        if busy {
                            ProgressView()
                        } else {
                            Image(systemName: "chevron.right")
                                .font(.footnote.weight(.semibold))
                                .foregroundStyle(.secondary)
                        }
                    }
                    .padding(.horizontal, 16)
                    .frame(height: 52)
                    .background(
                        RoundedRectangle(cornerRadius: 14, style: .continuous).fill(.white.opacity(0.08)))
                }
                .buttonStyle(PressStyle(scale: 0.97))
                .disabled(busy || (kind == .spotify && !model.spotify.isConfigured))
            }
            if let kind = model.activeKind, let message = model.source(kind).status.message {
                Label(message, systemImage: "info.circle")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            } else if !model.spotify.isConfigured, let message = model.spotify.status.message {
                Text("Spotify: \(message)")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
        }
        .foregroundStyle(.primary)
        .padding(20)
        .background(Panel())
    }
}
