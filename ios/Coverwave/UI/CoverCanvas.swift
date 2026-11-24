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

/// The character grid that fits a canvas: about 64 columns on a phone, 100
/// on an iPad, in SF Mono.
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

    init(size: CGSize) {
        let targetCols: CGFloat = size.width < 500 ? 64 : 100
        let fs = min(16, max(6, size.width / targetCols / Self.advancePerPoint))
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

    func frame(
        cover: AnalysedCover?, metrics: CellMetrics, mode: ArtMode, animate: Bool, date: Date,
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
        let tick = animate ? UInt32(truncatingIfNeeded: Int(date.timeIntervalSince(start) * 30)) : Self.stillTick
        if tick == lastTick, let last { return last }
        lastTick = tick
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
        for scalar in frame.glyphs.unicodeScalars {
            defer { i += 1 }
            let v = scalar.value
            guard v > 32, v < 128, i < frame.fg.count else { continue }
            let x = m.origin.x + CGFloat(i % cols) * m.cellWidth
            // Flipped below: y measured up from the bottom edge.
            let y = size.height - (m.origin.y + CGFloat(i / cols) * m.cellHeight + baseline)
            let key = frame.fg[i] & 0xFCFCFC
            runs[key, default: ([], [])].0.append(glyphs[Int(v)])
            runs[key, default: ([], [])].1.append(CGPoint(x: x, y: y))
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
        cg.restoreGState()
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
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !animate)) { timeline in
            Canvas(opaque: true, rendersAsynchronously: false) { context, size in
                let metrics = CellMetrics(size: size)
                let frame = renderer.frame(
                    cover: cover, metrics: metrics, mode: mode, animate: animate,
                    date: timeline.date
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
