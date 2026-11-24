import SwiftUI

/// The terminal look shared by every piece of chrome: SF Mono glyphs on the
/// page colour, no panels, no rounded cards. The one accent comes from the
/// cover (`.tint`).
enum Term {
    static let ink = Color(red: 228 / 255, green: 231 / 255, blue: 236 / 255)
    static let dim = Color(red: 128 / 255, green: 134 / 255, blue: 144 / 255)
    static let faint = Color(red: 72 / 255, green: 76 / 255, blue: 84 / 255)

    static func font(_ size: CGFloat = 15, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }

    /// Width of one glyph of `font(size)` (SF Mono advances 0.6 em).
    static func glyphWidth(_ size: CGFloat) -> CGFloat { size * 0.6 }
}

/// `1:42 ==========--------- 4:21`, sized to the width it is given. The
/// filled part takes the accent.
struct TermProgress: View {
    var fraction: Double
    var elapsed: TimeInterval
    var duration: TimeInterval
    var size: CGFloat = 13

    var body: some View {
        GeometryReader { geo in
            let left = timeText(elapsed)
            let right = duration > 0 ? timeText(duration) : "--:--"
            let cols = Int(geo.size.width / Term.glyphWidth(size)) - left.count - right.count - 2
            let n = max(4, cols)
            let filled = min(n, max(0, Int((fraction * Double(n)).rounded())))
            (Text(left + " ").foregroundColor(Term.dim)
                + Text(String(repeating: "=", count: filled)).foregroundStyle(.tint)
                + Text(String(repeating: "-", count: n - filled)).foregroundColor(Term.faint)
                + Text(" " + right).foregroundColor(Term.dim))
                .font(Term.font(size))
                .lineLimit(1)
                .fixedSize()
        }
        .frame(height: size * 1.4)
    }
}

/// A row in a terminal list: `title/ ... detail`, with the subtitle
/// dimmed. Highlights on touch-down.
struct TermRowStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .contentShape(Rectangle())
            .background(configuration.isPressed ? Term.faint.opacity(0.35) : .clear)
    }
}

/// Elapsed / remaining, as m:ss.
func timeText(_ seconds: TimeInterval) -> String {
    let s = max(0, Int(seconds.rounded(.down)))
    return String(format: "%d:%02d", s / 60, s % 60)
}
