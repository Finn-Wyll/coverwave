import SwiftUI
import UIKit

/// Haptics that land on the same frame as the visual change.
@MainActor
enum Haptics {
    private static let impact = UIImpactFeedbackGenerator(style: .light)
    private static let selection = UISelectionFeedbackGenerator()

    static func commit() { impact.impactOccurred() }
    static func tick() { selection.selectionChanged() }
}

/// Track, artist and an ASCII progress bar, bottom-left over a scrim of the
/// page colour. It shows while you touch, while paused, and briefly after
/// anything changes; the art is otherwise left alone.
struct HUD: View {
    let track: NowPlaying?
    let sourceName: String
    /// The device playing, when it is not this iPhone ("MacBook Pro").
    var elsewhere: String? = nil
    /// A one-line status: the last action, or why nothing is playing.
    let status: String?
    /// While scrubbing: the fraction under the finger.
    let scrub: Double?
    let onScrub: (Double?, Bool) -> Void

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.5)) { context in
            VStack(alignment: .leading, spacing: 6) {
                if let status {
                    Text(status)
                        .font(Term.font(13, .medium))
                        .foregroundStyle(.tint)
                        .transition(.opacity)
                }
                if let track {
                    let duration = track.duration ?? 0
                    let live = duration > 0 ? min(1, track.position(at: context.date) / duration) : 0
                    let shown = scrub ?? live
                    Text(track.title)
                        .font(Term.font(18, .semibold))
                        .foregroundStyle(Term.ink)
                    Text([track.artist, track.album].filter { !$0.isEmpty }.joined(separator: " / "))
                        .font(Term.font(13))
                        .foregroundStyle(Term.dim)
                    if let elsewhere {
                        Text("on \(elsewhere.lowercased()), swipe down to change")
                            .font(Term.font(12))
                            .foregroundStyle(.tint)
                    }
                    if scrub != nil {
                        Text(timeText(shown * duration))
                            .font(Term.font(40, .light))
                            .foregroundStyle(Term.ink)
                            .contentTransition(.numericText())
                            .padding(.top, 4)
                    }
                    TermProgress(fraction: shown, elapsed: shown * duration, duration: duration)
                        .padding(.top, 2)
                        .overlay {
                            GeometryReader { geo in
                                // The bar itself can be dragged in any scheme.
                                Color.clear.contentShape(Rectangle())
                                    .gesture(
                                        DragGesture(minimumDistance: 0)
                                            .onChanged { v in
                                                onScrub(min(1, max(0, v.location.x / max(1, geo.size.width))), false)
                                            }
                                            .onEnded { v in
                                                onScrub(min(1, max(0, v.location.x / max(1, geo.size.width))), true)
                                            })
                            }
                            .padding(.vertical, -12)
                        }
                        .accessibilityElement(children: .ignore)
                        .accessibilityLabel("Position")
                        .accessibilityValue("\(timeText(shown * duration)) of \(timeText(duration))")
                } else {
                    Text("nothing playing")
                        .font(Term.font(18, .semibold))
                        .foregroundStyle(Term.ink)
                    Text("start something in \(sourceName), or swipe up for your library")
                        .font(Term.font(13))
                        .foregroundStyle(Term.dim)
                }
            }
            .lineLimit(1)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 20)
        .padding(.top, 48)
        .padding(.bottom, 12)
        .background(alignment: .bottom) {
            LinearGradient(
                colors: [Theme.page.opacity(0), Theme.page.opacity(0.88)],
                startPoint: .top, endPoint: .bottom
            )
            .ignoresSafeArea()
            .allowsHitTesting(false)
        }
    }
}

/// The dial's options, by direction from the press.
enum DialOption: CaseIterable {
    case library, queue, previous, next

    var label: String {
        switch self {
        case .library: "library"
        case .queue: "queue"
        case .previous: "prev"
        case .next: "next"
        }
    }

    var offset: CGSize {
        let r: CGFloat = 86
        return switch self {
        case .library: CGSize(width: 0, height: -r)
        case .queue: CGSize(width: 0, height: r)
        case .previous: CGSize(width: -r, height: 0)
        case .next: CGSize(width: r, height: 0)
        }
    }

    /// The option a finger `d` away from the press points at (nil inside
    /// the dead zone).
    static func at(_ d: CGSize) -> DialOption? {
        guard hypot(d.width, d.height) > 34 else { return nil }
        if abs(d.width) > abs(d.height) { return d.width < 0 ? .previous : .next }
        return d.height < 0 ? .library : .queue
    }
}

/// Four labels around the finger, in glyphs; the one pointed at is
/// bracketed in the accent.
struct DialView: View {
    let selected: DialOption?

    var body: some View {
        ZStack {
            Text("+")
                .font(Term.font(20))
                .foregroundStyle(Term.dim)
            ForEach(DialOption.allCases, id: \.self) { option in
                let on = option == selected
                Text(on ? "[\(option.label)]" : option.label)
                    .font(Term.font(15, on ? .semibold : .regular))
                    .foregroundStyle(on ? AnyShapeStyle(.tint) : AnyShapeStyle(Term.ink))
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3)
                    .background(Theme.page.opacity(0.82))
                    .offset(option.offset)
                    .scaleEffect(on ? 1.12 : 1)
            }
        }
        .animation(.spring(response: 0.22, dampingFraction: 1), value: selected)
        .accessibilityHidden(true)
    }
}

/// The cheat sheet for the chosen scheme, shown once and from Settings.
struct GestureGuide: View {
    let scheme: GestureScheme
    let dismiss: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            VStack(alignment: .leading, spacing: 4) {
                Text("gestures: \(scheme.label.lowercased())")
                    .font(Term.font(18, .semibold))
                    .foregroundStyle(Term.ink)
                Text("the art is the control. change the scheme in settings.")
                    .font(Term.font(13))
                    .foregroundStyle(Term.dim)
            }
            GuideTable(scheme: scheme)
            Text("tap anywhere to start")
                .font(Term.font(13, .medium))
                .foregroundStyle(.tint)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        .background(Theme.page.opacity(0.94))
        .contentShape(Rectangle())
        .onTapGesture(perform: dismiss)
        .accessibilityAddTraits(.isButton)
        .accessibilityAction(named: "Start", dismiss)
    }
}

struct GuideTable: View {
    let scheme: GestureScheme

    var body: some View {
        let lines = scheme.guide
        let width = (lines.map(\.gesture.count).max() ?? 0) + 2
        VStack(alignment: .leading, spacing: 6) {
            ForEach(lines, id: \.gesture) { line in
                (Text(line.gesture.padding(toLength: width, withPad: " ", startingAt: 0))
                    .foregroundColor(Term.dim)
                    + Text(line.action).foregroundColor(Term.ink))
                    .font(Term.font(14))
            }
        }
        .accessibilityElement(children: .combine)
    }
}
