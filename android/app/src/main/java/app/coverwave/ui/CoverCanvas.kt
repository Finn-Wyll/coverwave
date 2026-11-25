package app.coverwave.ui

import android.graphics.Paint
import android.graphics.Typeface
import android.os.Build
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import app.coverwave.AppModel
import app.coverwave.ArtDetail
import uniffi.cw_ffi.AnalysedCover
import uniffi.cw_ffi.Animator
import uniffi.cw_ffi.ArtMode
import uniffi.cw_ffi.AudioLevels
import uniffi.cw_ffi.CellFrame
import uniffi.cw_ffi.LyricPaintKind
import uniffi.cw_ffi.LyricScrollKind
import uniffi.cw_ffi.LyricSheet
import uniffi.cw_ffi.LyricSizeKind
import uniffi.cw_ffi.LyricStyleKind
import uniffi.cw_ffi.idleFrame
import uniffi.cw_ffi.noBackground

/** The monospaced face the art is drawn in (semibold, like the iPhone's). */
private val artTypeface: Typeface =
    if (Build.VERSION.SDK_INT >= 28) Typeface.create(Typeface.MONOSPACE, 600, false)
    else Typeface.create(Typeface.MONOSPACE, Typeface.BOLD)

/**
 * The character grid that fits a canvas: 64, 96 or 128 columns on a phone
 * (by [ArtDetail]), 1.5 times that on a tablet. Sizes in pixels.
 */
data class CellMetrics(
    val fontSize: Float,
    val cellWidth: Float,
    val cellHeight: Float,
    val cols: Int,
    val rows: Int,
    val originX: Float,
    val originY: Float,
) {
    /** Cell width / height, as the core expects. */
    val cellAspect: Float get() = cellWidth / cellHeight

    companion object {
        private val advancePerPx: Float by lazy {
            Paint().apply { typeface = artTypeface; textSize = 100f }.measureText("M") / 100f
        }

        fun of(width: Float, height: Float, density: Float, detail: ArtDetail): CellMetrics {
            val tablet = width / density >= 500
            val target = detail.phoneColumns * if (tablet) 1.5f else 1f
            // 4pt is still 12 pixels tall on a 3x screen.
            val fs = (width / target / advancePerPx).coerceIn(4f * density, 16f * density)
            val cw = fs * advancePerPx
            val ch = fs * 1.2f
            val cols = maxOf(1, (width / cw).toInt()).coerceAtMost(65535)
            val rows = maxOf(1, (height / ch).toInt()).coerceAtMost(65535)
            return CellMetrics(fs, cw, ch, cols, rows, (width - cols * cw) / 2, (height - rows * ch) / 2)
        }
    }
}

/** Lyrics to draw over the cover for one frame. */
data class LyricsInput(
    val sheet: LyricSheet,
    val position: Double,
    val duration: Double?,
    val style: LyricStyleKind,
    val scroll: LyricScrollKind,
    val size: LyricSizeKind,
    val paint: LyricPaintKind,
)

/**
 * Owns the core's animator for the current cover and size, and hands out
 * one frame per 30fps tick.
 */
class CoverRenderer {
    val painter = CellPainter()
    private data class Key(val cover: AnalysedCover?, val metrics: CellMetrics, val mode: ArtMode)

    private var key: Key? = null
    private var animator: Animator? = null
    private var lastTick: Long? = null
    private var last: CellFrame? = null
    private val start = System.currentTimeMillis()

    fun frame(
        cover: AnalysedCover?,
        metrics: CellMetrics,
        mode: ArtMode,
        animate: Boolean,
        nowMs: Long,
        lyrics: (Long) -> LyricsInput?,
        levels: (Long) -> AudioLevels,
    ): CellFrame {
        val k = Key(cover, metrics, mode)
        if (k != key) {
            key = k
            lastTick = null
            animator?.close()
            animator = cover?.let {
                Animator(it, metrics.cols.toUShort(), metrics.rows.toUShort(), metrics.cellAspect, mode, Theme.pageCore)
            }
        }
        val words = if (animator == null) null else lyrics(nowMs)
        // Lyrics scroll even with motion off, so they need a moving clock.
        val tick = if (animate || words != null) (nowMs - start) * 30 / 1000 else -1L
        if (tick == lastTick) last?.let { return it }
        lastTick = tick
        val a = animator
        val f = when {
            a != null && words != null -> a.frameWithLyrics(
                tick.toUInt(), if (animate) levels(nowMs) else AppModel.SILENCE, words.sheet,
                words.position, words.duration, words.style, words.scroll, words.size, words.paint, animate,
            )
            a != null && animate -> a.frame(tick.toUInt(), levels(nowMs))
            a != null -> a.still()
            else -> idleFrame(
                metrics.cols.toUShort(), metrics.rows.toUShort(), if (animate) tick.toUInt() else 0u,
                0x636F7665uL, Theme.pageCore,
            )
        }
        last = f
        return f
    }
}

/**
 * Draws a [CellFrame]: glyphs grouped by colour, one `drawPosText` call per
 * colour, each glyph at its exact cell position.
 */
class CellPainter {
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply { typeface = artTypeface }
    private val fill = Paint()

    /** Per colour: glyphs and their positions, reused frame to frame. */
    private class Run {
        var chars = CharArray(256)
        var pos = FloatArray(512)
        var n = 0
        fun add(c: Char, x: Float, y: Float) {
            if (n == chars.size) {
                chars = chars.copyOf(n * 2)
                pos = pos.copyOf(n * 4)
            }
            chars[n] = c
            pos[2 * n] = x
            pos[2 * n + 1] = y
            n++
        }
    }

    private val runs = HashMap<Int, Run>()
    private val none = noBackground().toInt()

    @Suppress("DEPRECATION")
    fun draw(canvas: android.graphics.Canvas, frame: CellFrame, m: CellMetrics) {
        canvas.drawColor(0xFF000000.toInt() or Theme.PAGE_PACKED)
        val cols = frame.cols.toInt()
        if (cols == 0) return
        paint.textSize = m.fontSize
        val fm = paint.fontMetrics
        val baseline = (m.cellHeight - (fm.descent - fm.ascent)) / 2 - fm.ascent

        // Backgrounds that differ from the page.
        val bg = frame.bg
        for (i in bg.indices) {
            val b = bg[i].toInt()
            if (b == none || b == Theme.PAGE_PACKED) continue
            fill.color = 0xFF000000.toInt() or b
            val x = m.originX + (i % cols) * m.cellWidth
            val y = m.originY + (i / cols) * m.cellHeight
            canvas.drawRect(x, y, x + m.cellWidth, y + m.cellHeight, fill)
        }

        // Glyphs, grouped by colour. The two lowest bits of each channel are
        // dropped: invisible, and it keeps true-colour frames to fewer draws.
        for (r in runs.values) r.n = 0
        val glyphs = frame.glyphs
        val fg = frame.fg
        var i = 0
        var at = 0
        while (at < glyphs.length && i < fg.size) {
            val cp = glyphs.codePointAt(at)
            at += Character.charCount(cp)
            if (cp > 32) {
                val x = m.originX + (i % cols) * m.cellWidth
                val y = m.originY + (i / cols) * m.cellHeight + baseline
                val key = fg[i].toInt() and 0xFCFCFC
                if (cp < 0x10000) {
                    runs.getOrPut(key) { Run() }.add(cp.toChar(), x, y)
                } else {
                    // Beyond the BMP (rare, in lyrics): drawn on its own.
                    paint.color = 0xFF000000.toInt() or key
                    canvas.drawText(String(Character.toChars(cp)), x, y, paint)
                }
            }
            i++
        }
        for ((key, r) in runs) {
            if (r.n == 0) continue
            paint.color = 0xFF000000.toInt() or key
            canvas.drawPosText(r.chars, 0, r.n, r.pos, paint)
        }
        if (runs.size > 4096) runs.clear()
    }
}

/** The cover as moving ASCII art, or the idle scene when nothing plays. */
@Composable
fun CoverCanvas(model: AppModel, animate: Boolean, modifier: Modifier = Modifier) {
    val renderer = remember { CoverRenderer() }
    val density = LocalDensity.current.density
    val cover = model.cover
    val settings = model.settings
    val mode = settings.artMode
    val detail = settings.artDetail
    val lyricsOn = settings.showLyrics && model.lyricSheet != null
    var tick by remember { mutableLongStateOf(0L) }
    LaunchedEffect(animate, lyricsOn) {
        if (!animate && !lyricsOn) return@LaunchedEffect
        while (true) {
            withFrameMillis { ms ->
                val t = ms * 30 / 1000
                if (t != tick) tick = t
            }
        }
    }
    Canvas(modifier.fillMaxSize().semantics { contentDescription = if (cover == null) "coverwave" else "Album cover" }) {
        @Suppress("UNUSED_EXPRESSION") tick // redraw on each 30fps tick
        val m = CellMetrics.of(size.width, size.height, density, detail)
        val frame = renderer.frame(
            cover, m, mode, animate, System.currentTimeMillis(),
            lyrics = { now ->
                val sheet = model.lyricSheet
                val np = model.nowPlaying
                if (!lyricsOn || sheet == null || np == null) null
                else LyricsInput(
                    sheet, np.positionAt(now), np.duration, settings.lyricStyle,
                    settings.lyricScroll, settings.lyricSize, settings.lyricPaint,
                )
            },
            levels = { now -> model.levels(now) },
        )
        drawIntoCanvas { renderer.painter.draw(it.nativeCanvas, frame, m) }
    }
}
