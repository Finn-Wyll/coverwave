import CoreText
import SwiftUI
import UIKit

enum Theme {
    /// The page behind the art: the terminal app's background.
    static let page = Color(red: 14 / 255, green: 15 / 255, blue: 18 / 255)
    static let pageCore = CoverColor(r: 14, g: 15, b: 18)
    static let pagePacked: UInt32 = 0x0E0F12
    /// Accent until a cover provides one.
    static let defaultAccent = Color(red: 196 / 255, green: 204 / 255, blue: 214 / 255)
}

/// How many characters the art is made of (see `CellMetrics`). Finer
/// detail means smaller glyphs: the cover reads more like the picture.
/// How many characters the art is made of, coarse to fine (a slider in
/// Settings). Saved by raw value, so the first three keep their old names.
enum ArtDetail: String, CaseIterable, Identifiable {
    case standard
    case fine
    case finest
    case extraFine
    case ultraFine
    case maximum

    var id: String { rawValue }

    var label: String {
        switch self {
        case .standard: "Standard"
        case .fine: "Fine"
        case .finest: "Finest"
        case .extraFine: "Extra Fine"
        case .ultraFine: "Ultra Fine"
        case .maximum: "Maximum"
        }
    }

    /// Columns across a phone; an iPad gets about 1.5 times as many.
    var phoneColumns: CGFloat {
        switch self {
        case .standard: 64
        case .fine: 96
        case .finest: 128
        case .extraFine: 160
        case .ultraFine: 192
        case .maximum: 224
        }
    }

    /// Its place on the slider (0 coarsest).
    var step: Int { Self.allCases.firstIndex(of: self) ?? 0 }
}

/// The character grid that fits a canvas, in SF Mono: 64 to 224 columns on
/// a phone (by `ArtDetail`), 1.5 times that on an iPad.
struct CellMetrics: Equatable {
    let fontSize: CGFloat
    let cellWidth: CGFloat
    let cellHeight: CGFloat
    let cols: UInt16
    let rows: UInt16
    let origin: CGPoint

    /// Cell width / height, as the core expects.
    var cellAspect: Float { Float(cellWidth / cellHeight) }

    private static let advancePerPoint: CGFloat = {
        let font = UIFont.monospacedSystemFont(ofSize: 100, weight: .semibold)
        return ("M" as NSString).size(withAttributes: [.font: font]).width / 100
    }()

    init(size: CGSize, detail: ArtDetail = .fine) {
        let targetCols = detail.phoneColumns * (size.width < 500 ? 1 : 1.5)
        // 2.5pt is still 9 pixels tall on a 3x screen: at the finest levels
        // the characters read as texture, like pixels.
        let fs = min(16, max(2.5, size.width / targetCols / Self.advancePerPoint))
        fontSize = fs
        cellWidth = fs * Self.advancePerPoint
        cellHeight = fs * 1.2
        cols = UInt16(clamping: max(1, Int(size.width / cellWidth)))
        rows = UInt16(clamping: max(1, Int(size.height / cellHeight)))
        origin = CGPoint(
            x: (size.width - CGFloat(cols) * cellWidth) / 2,
            y: (size.height - CGFloat(rows) * cellHeight) / 2)
    }
}

/// Owns the core's animator for the current cover and size, and hands out
/// one frame per 30fps tick.
@MainActor
final class CoverRenderer {
    let painter = CellPainter()
    private struct Key: Equatable {
        let cover: ObjectIdentifier?
        let metrics: CellMetrics
        let mode: ArtMode
    }

    private var key: Key?
    private var animator: Animator?
    private var lastTick: UInt32?
    private var last: CellFrame?
    private let start = Date()
    /// Tick used for still frames (motion off).
    private static let stillTick = UInt32.max

    /// Lyrics to draw over the cover for one frame.
    struct LyricsInput {
        let sheet: LyricSheet
        let position: TimeInterval
        let duration: TimeInterval?
        let style: LyricStyleKind
        let scroll: LyricScrollKind
        let size: LyricSizeKind
        let paint: LyricPaintKind
    }

    func frame(
        cover: AnalysedCover?, metrics: CellMetrics, mode: ArtMode, animate: Bool, date: Date,
        lyrics: (Date) -> LyricsInput? = { _ in nil },
        levels: (Date) -> AudioLevels
    ) -> CellFrame {
        let key = Key(cover: cover.map { ObjectIdentifier($0) }, metrics: metrics, mode: mode)
        if key != self.key {
            self.key = key
            lastTick = nil
            animator = cover.map {
                Animator(
                    cover: $0, cols: metrics.cols, rows: metrics.rows,
                    cellAspect: metrics.cellAspect, mode: mode, background: Theme.pageCore)
            }
        }
        let words = animator == nil ? nil : lyrics(date)
        // Lyrics scroll even with motion off, so they need a moving clock.
        let tick =
            animate || words != nil
            ? UInt32(truncatingIfNeeded: Int(date.timeIntervalSince(start) * 30)) : Self.stillTick
        if tick == lastTick, let last { return last }
        lastTick = tick
        if let a = animator, let w = words {
            let frame = a.frameWithLyrics(
                frame: tick, audio: animate ? levels(date) : AudioLevels(
                    bands: Array(repeating: 0, count: 6), level: 0, onset: 0, onsetCount: 0),
                lyrics: w.sheet, position: w.position, duration: w.duration,
                style: w.style, scroll: w.scroll, size: w.size, paint: w.paint, animate: animate)
            last = frame
            return frame
        }
        let frame: CellFrame
        switch (animator, animate) {
        case (let a?, true): frame = a.frame(frame: tick, audio: levels(date))
        case (let a?, false): frame = a.still()
        case (nil, _):
            frame = idleFrame(
                cols: metrics.cols, rows: metrics.rows, frame: animate ? tick : 0,
                seed: 0x636F_7665, background: Theme.pageCore)
        }
        last = frame
        return frame
    }
}

/// Draws a `CellFrame` with Core Text: glyphs grouped by colour, one draw
/// call per colour, each glyph at its exact cell position.
@MainActor
final class CellPainter {
    private var font: CTFont?
    private var fontSize: CGFloat = 0
    private var ascent: CGFloat = 0
    private var descent: CGFloat = 0
    private var glyphs = [CGGlyph](repeating: 0, count: 128)
    private var colors: [UInt32: CGColor] = [:]

    private func prepareFont(_ size: CGFloat) -> CTFont {
        if let font, size == fontSize { return font }
        let f = UIFont.monospacedSystemFont(ofSize: size, weight: .semibold) as CTFont
        font = f
        extraGlyphs.removeAll()
        fontSize = size
        ascent = CTFontGetAscent(f)
        descent = CTFontGetDescent(f)
        var chars = (0..<128).map { UniChar($0) }
        glyphs = [CGGlyph](repeating: 0, count: 128)
        CTFontGetGlyphsForCharacters(f, &chars, &glyphs, 128)
        return f
    }

    private func color(_ packed: UInt32) -> CGColor {
        if let c = colors[packed] { return c }
        let c = CGColor(
            srgbRed: CGFloat(packed >> 16 & 0xFF) / 255, green: CGFloat(packed >> 8 & 0xFF) / 255,
            blue: CGFloat(packed & 0xFF) / 255, alpha: 1)
        if colors.count > 8192 { colors.removeAll(keepingCapacity: true) }
        colors[packed] = c
        return c
    }

    func draw(_ frame: CellFrame, metrics m: CellMetrics, in cg: CGContext, size: CGSize) {
        let font = prepareFont(m.fontSize)
        cg.setFillColor(color(Theme.pagePacked))
        cg.fill(CGRect(origin: .zero, size: size))
        let cols = Int(frame.cols)
        let none = noBackground()

        // Backgrounds that differ from the page.
        var rects: [UInt32: [CGRect]] = [:]
        for (i, bg) in frame.bg.enumerated() where bg != none && bg != Theme.pagePacked {
            let x = m.origin.x + CGFloat(i % cols) * m.cellWidth
            let y = m.origin.y + CGFloat(i / cols) * m.cellHeight
            rects[bg, default: []].append(CGRect(x: x, y: y, width: m.cellWidth, height: m.cellHeight))
        }
        for (packed, list) in rects {
            cg.setFillColor(color(packed))
            cg.fill(list)
        }

        // Glyphs, grouped by colour. The two lowest bits of each channel are
        // dropped: invisible, and it keeps true-colour frames to fewer draws.
        var runs: [UInt32: ([CGGlyph], [CGPoint])] = [:]
        let baseline = (m.cellHeight - ascent - descent) / 2 + ascent
        var i = 0
        // Lyrics bring characters beyond ASCII (accents, curly quotes,
        // other scripts): drawn one by one in a font that has them.
        var others: [(CTFont, CGGlyph, CGPoint, UInt32)] = []
        for scalar in frame.glyphs.unicodeScalars {
            defer { i += 1 }
            let v = scalar.value
            guard v > 32, i < frame.fg.count else { continue }
            let x = m.origin.x + CGFloat(i % cols) * m.cellWidth
            // Flipped below: y measured up from the bottom edge.
            let y = size.height - (m.origin.y + CGFloat(i / cols) * m.cellHeight + baseline)
            let key = frame.fg[i] & 0xFCFCFC
            if v < 128 {
                runs[key, default: ([], [])].0.append(glyphs[Int(v)])
                runs[key, default: ([], [])].1.append(CGPoint(x: x, y: y))
            } else if let (f, g) = glyph(for: scalar, font: font) {
                others.append((f, g, CGPoint(x: x, y: y), key))
            }
        }
        cg.saveGState()
        cg.textMatrix = .identity
        cg.translateBy(x: 0, y: size.height)
        cg.scaleBy(x: 1, y: -1)
        for (packed, run) in runs {
            cg.setFillColor(color(packed))
            run.0.withUnsafeBufferPointer { g in
                run.1.withUnsafeBufferPointer { p in
                    CTFontDrawGlyphs(font, g.baseAddress!, p.baseAddress!, g.count, cg)
                }
            }
        }
        for (f, g, p, packed) in others {
            cg.setFillColor(color(packed))
            var glyph = g
            var point = p
            CTFontDrawGlyphs(f, &glyph, &point, 1, cg)
        }
        cg.restoreGState()
    }

    /// Fonts and glyphs for characters beyond ASCII, found once each: SF
    /// Mono when it has them, else the system's fallback for that text.
    private var extraGlyphs: [UInt32: (CTFont, CGGlyph)?] = [:]

    private func glyph(for scalar: Unicode.Scalar, font: CTFont) -> (CTFont, CGGlyph)? {
        if let found = extraGlyphs[scalar.value] { return found }
        let units = Array(String(scalar).utf16)
        var gs = [CGGlyph](repeating: 0, count: units.count)
        var found: (CTFont, CGGlyph)?
        if CTFontGetGlyphsForCharacters(font, units, &gs, units.count), gs[0] != 0 {
            found = (font, gs[0])
        } else {
            let text = String(scalar) as CFString
            let fallback = CTFontCreateForString(font, text, CFRange(location: 0, length: units.count))
            if CTFontGetGlyphsForCharacters(fallback, units, &gs, units.count), gs[0] != 0 {
                found = (fallback, gs[0])
            }
        }
        if extraGlyphs.count > 4096 { extraGlyphs.removeAll() }
        extraGlyphs[scalar.value] = .some(found)
        return found
    }
}

/// The cover as moving ASCII art, or the idle scene when nothing plays.
struct CoverCanvas: View {
    @Environment(AppModel.self) private var model
    let animate: Bool
    @State private var renderer = CoverRenderer()

    var body: some View {
        let cover = model.cover
        let mode = model.settings.artMode
        let detail = model.settings.artDetail
        let lyricsOn = model.settings.showLyrics && model.lyricSheet != nil
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !animate && !lyricsOn)) { timeline in
            Canvas(opaque: true, rendersAsynchronously: false) { context, size in
                let metrics = CellMetrics(size: size, detail: detail)
                let frame = renderer.frame(
                    cover: cover, metrics: metrics, mode: mode, animate: animate,
                    date: timeline.date,
                    lyrics: { date in
                        guard lyricsOn, let sheet = model.lyricSheet, let np = model.nowPlaying else { return nil }
                        return .init(
                            sheet: sheet, position: np.position(at: date), duration: np.duration,
                            style: model.settings.lyricStyle, scroll: model.settings.lyricScroll,
                            size: model.settings.lyricSize, paint: model.settings.lyricPaint)
                    }
                ) { model.levels(at: $0) }
                context.withCGContext { cg in
                    renderer.painter.draw(frame, metrics: metrics, in: cg, size: size)
                }
            }
        }
        .background(Theme.page)
        .accessibilityElement()
        .accessibilityLabel(cover == nil ? "coverwave" : "Album cover")
        .accessibilityAddTraits(.isImage)
    }
}
