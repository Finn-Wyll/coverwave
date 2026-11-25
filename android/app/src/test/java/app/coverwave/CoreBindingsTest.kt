package app.coverwave

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.BeforeClass
import org.junit.Test
import uniffi.cw_ffi.Animator
import uniffi.cw_ffi.ArtMode
import uniffi.cw_ffi.CoverColor
import uniffi.cw_ffi.CoverEngine
import uniffi.cw_ffi.LyricPaintKind
import uniffi.cw_ffi.LyricScrollKind
import uniffi.cw_ffi.LyricSizeKind
import uniffi.cw_ffi.LyricStyleKind
import uniffi.cw_ffi.beatLevels
import uniffi.cw_ffi.genreFamily
import uniffi.cw_ffi.idleFrame
import uniffi.cw_ffi.parseLyrics
import java.io.File

/**
 * Runs the generated Kotlin bindings against a host build of the Rust core
 * (`cargo build -p cw-ffi --release`), so the Kotlin <-> Rust boundary the
 * app uses is checked without a device. Skipped when that library is
 * missing.
 */
class CoreBindingsTest {
    companion object {
        @BeforeClass
        @JvmStatic
        fun findCore() {
            val lib = File("../../target/release/libcw_ffi.so").absoluteFile
            org.junit.Assume.assumeTrue("host core not built", lib.exists())
            System.setProperty("jna.library.path", lib.parent)
        }
    }

    /** The README's demo cover. */
    private fun cover(): ByteArray = File("../../docs/media/demo-cover.jpg").readBytes()

    @Test
    fun coverAnalysesAndAnimatesDeterministically() {
        val engine = CoverEngine()
        assertFalse(engine.hasModel())
        val a = engine.analyse(cover(), "House", null)
        val b = engine.analyse(cover(), "House", null)
        assertEquals(a.recipeJson(), b.recipeJson())
        assertEquals("electronic", a.genreFamily())
        assertTrue(a.layerKinds().isNotEmpty())
        assertEquals(64, a.hash().length)
        val bg = CoverColor(14u, 15u, 18u)
        val anim1 = Animator(a, 60u, 30u, 0.5f, ArtMode.PALETTE, bg)
        val anim2 = Animator(b, 60u, 30u, 0.5f, ArtMode.PALETTE, bg)
        for (f in 0u until 40u) {
            val lv = beatLevels(124f, "House", f.toDouble() / 30, true)
            val x = anim1.frame(f, lv)
            assertEquals(x, anim2.frame(f, lv))
            assertEquals(60 * 30, x.glyphs.codePointCount(0, x.glyphs.length))
            assertEquals(60 * 30, x.fg.size)
        }
    }

    @Test
    fun lyricsIdleAndGenres() {
        val sheet = parseLyrics("[00:01.00]hello there\n[00:03.00]second line")
        assertNotNull(sheet)
        assertTrue(sheet!!.isSynced())
        val engine = CoverEngine()
        val anim = Animator(engine.analyse(cover(), null, null), 80u, 40u, 0.5f, ArtMode.TRUE_COLOR, CoverColor(0u, 0u, 0u))
        val f = anim.frameWithLyrics(
            10u, beatLevels(0f, null, 2.0, true), sheet, 2.0, 10.0,
            LyricStyleKind.BIG_ONLY, LyricScrollKind.UP, LyricSizeKind.MEDIUM, LyricPaintKind.ACCENT, true,
        )
        assertEquals(80 * 40, f.fg.size)
        assertEquals("metal", genreFamily("Death Metal"))
        assertEquals(800, idleFrame(40u, 20u, 0u, 7uL, CoverColor(0u, 0u, 0u)).fg.size)
    }
}
