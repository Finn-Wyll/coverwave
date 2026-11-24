import SwiftUI

/// Feedback on touch-down, not on release: a quick press-in that springs
/// back without overshoot.
struct PressStyle: ButtonStyle {
    var scale: CGFloat = 0.88

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .contentShape(Rectangle())
            .scaleEffect(configuration.isPressed ? scale : 1)
            .opacity(configuration.isPressed ? 0.75 : 1)
            .animation(.spring(response: 0.25, dampingFraction: 1), value: configuration.isPressed)
    }
}

/// The floating material behind chrome. A heavier material, so text stays
/// legible over busy art; solid when Reduce Transparency is on.
struct Panel: View {
    var cornerRadius: CGFloat = 28
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
    @Environment(\.colorSchemeContrast) private var contrast

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
        ZStack {
            if reduceTransparency {
                shape.fill(Color(white: 0.11))
            } else {
                shape.fill(.regularMaterial)
            }
            // A faint top-lit edge, like light catching the material.
            shape.strokeBorder(
                LinearGradient(
                    colors: [.white.opacity(contrast == .increased ? 0.5 : 0.14), .white.opacity(0.03)],
                    startPoint: .top, endPoint: .bottom),
                lineWidth: contrast == .increased ? 1 : 0.5)
        }
        .shadow(color: .black.opacity(0.35), radius: cornerRadius > 20 ? 24 : 10, y: 8)
    }
}

/// Elapsed / remaining, as m:ss.
func timeText(_ seconds: TimeInterval) -> String {
    let s = max(0, Int(seconds.rounded(.down)))
    return String(format: "%d:%02d", s / 60, s % 60)
}

/// The progress bar. Follows the finger 1:1 while dragged (and thickens
/// under it), seeks on release, and holds the new position until the
/// source catches up.
struct Scrubber: View {
    let track: NowPlaying
    let onSeek: (TimeInterval) -> Void

    @State private var dragFraction: Double?
    @State private var pending: (fraction: Double, at: Date)?

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.5)) { context in
            let duration = track.duration ?? 0
            let live = duration > 0 ? min(1, track.position(at: context.date) / duration) : 0
            let shown = dragFraction ?? heldFraction(at: context.date) ?? live
            VStack(spacing: 4) {
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(.white.opacity(0.16))
                        Capsule().fill(.tint).frame(width: max(0, geo.size.width * shown))
                    }
                    .frame(height: dragFraction == nil ? 5 : 10)
                    .frame(maxHeight: .infinity)
                    .contentShape(Rectangle())
                    .gesture(
                        DragGesture(minimumDistance: 0)
                            .onChanged { value in
                                dragFraction = min(1, max(0, value.location.x / max(1, geo.size.width)))
                            }
                            .onEnded { _ in
                                if let f = dragFraction, duration > 0 {
                                    pending = (f, .now)
                                    onSeek(f * duration)
                                }
                                dragFraction = nil
                            }
                    )
                    .disabled(duration == 0)
                }
                .frame(height: 22)
                HStack {
                    Text(timeText(shown * duration))
                    Spacer()
                    Text(duration > 0 ? "-" + timeText(duration * (1 - shown)) : "")
                }
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
            }
            .animation(.spring(response: 0.3, dampingFraction: 1), value: dragFraction == nil)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Position")
            .accessibilityValue("\(timeText(shown * duration)) of \(timeText(duration))")
            .accessibilityAdjustableAction { direction in
                let step: TimeInterval = direction == .increment ? 10 : -10
                onSeek(min(duration, max(0, shown * duration + step)))
            }
        }
    }

    /// The position just seeked to, for a moment, so the bar does not jump
    /// back before the source reports it.
    private func heldFraction(at date: Date) -> Double? {
        guard let pending, date.timeIntervalSince(pending.at) < 1.5 else { return nil }
        return pending.fraction
    }
}

/// Previous, play/pause, next. A light tap of haptics on play/pause only.
struct TransportControls: View {
    let source: any MusicSource
    let isPlaying: Bool
    let enabled: Bool
    @State private var toggles = 0

    var body: some View {
        HStack {
            Spacer()
            Button { source.previous() } label: {
                Image(systemName: "backward.fill").font(.title2).frame(width: 56, height: 56)
            }
            .accessibilityLabel("Previous")
            Spacer()
            Button {
                toggles += 1
                source.playPause()
            } label: {
                Image(systemName: isPlaying ? "pause.fill" : "play.fill")
                    .font(.system(size: 38))
                    .contentTransition(.symbolEffect(.replace))
                    .frame(width: 68, height: 68)
            }
            .accessibilityLabel(isPlaying ? "Pause" : "Play")
            Spacer()
            Button { source.next() } label: {
                Image(systemName: "forward.fill").font(.title2).frame(width: 56, height: 56)
            }
            .accessibilityLabel("Next")
            Spacer()
        }
        .foregroundStyle(.primary)
        .buttonStyle(PressStyle())
        .disabled(!enabled)
        .opacity(enabled ? 1 : 0.4)
        .sensoryFeedback(.impact(weight: .light), trigger: toggles)
    }
}
