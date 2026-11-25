package app.coverwave

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import app.coverwave.core.Platform
import app.coverwave.sources.SourceKind
import uniffi.cw_ffi.ArtMode
import uniffi.cw_ffi.LyricPaintKind
import uniffi.cw_ffi.LyricScrollKind
import uniffi.cw_ffi.LyricSizeKind
import uniffi.cw_ffi.LyricStyleKind
import kotlin.reflect.KProperty

enum class MotionSetting(val label: String) { AUTOMATIC("Automatic"), ON("On"), OFF("Off") }

/** How many characters the art is made of. */
enum class ArtDetail(val label: String, val phoneColumns: Float) {
    STANDARD("Standard", 64f),
    FINE("Fine", 96f),
    FINEST("Finest", 128f),
}

/**
 * How touches on the art control playback. Swipes are the same in every
 * scheme (sideways: next/previous, up: library, down: queue, two-finger
 * tap: settings); the schemes differ in what a tap and a hold do.
 */
enum class GestureScheme(val label: String) {
    /** Tap: play/pause. Hold and slide: scrub. */
    DIRECT("Direct"),

    /** Tap the left, middle or right third: previous, play/pause, next. */
    ZONES("Zones"),

    /** Tap: play/pause. Hold: a dial under the finger. */
    DIAL("Dial");

    /** The cheat sheet, one gesture per line. */
    val guide: List<Pair<String, String>>
        get() = when (this) {
            DIRECT -> listOf("tap" to "play / pause", "hold + slide" to "scrub")
            ZONES -> listOf(
                "tap left" to "previous", "tap middle" to "play / pause", "tap right" to "next",
                "hold + slide" to "scrub",
            )
            DIAL -> listOf("tap" to "play / pause", "hold" to "dial: slide, let go")
        } + listOf(
            "swipe left" to "next", "swipe right" to "previous",
            "swipe up" to "library", "swipe down" to "queue",
            "two-finger tap" to "settings", "two-finger swipe" to "lyrics / cover",
        )
}

/** What to save ahead for offline listening. */
enum class SaveScope(val label: String) { OFF("Off"), LIKED("Liked Songs"), ALL("Liked + Playlists") }

val ArtMode.label: String
    get() = when (this) {
        ArtMode.PALETTE -> "Palette"
        ArtMode.TRUE_COLOR -> "True colour"
        ArtMode.DUOTONE -> "Duotone"
    }
val LyricStyleKind.label: String
    get() = when (this) {
        LyricStyleKind.BIG_CURRENT -> "Big Line"
        LyricStyleKind.TEXT -> "Text"
        LyricStyleKind.BIG_ONLY -> "Big Only"
    }
val LyricScrollKind.label: String get() = if (this == LyricScrollKind.UP) "Up" else "Down"
val LyricSizeKind.label: String
    get() = when (this) {
        LyricSizeKind.SMALL -> "Small"
        LyricSizeKind.MEDIUM -> "Medium"
        LyricSizeKind.LARGE -> "Large"
    }

/** User choices, kept in SharedPreferences (same names and defaults as iOS). */
class AppSettings {
    private val prefs = Platform.prefs

    /** A Compose state saved under [key]. */
    private inner class Saved<T>(
        private val key: String,
        private val read: (String?) -> T,
        private val write: (T) -> String?,
    ) {
        private var state = mutableStateOf(read(prefs.getString(key, null)))
        operator fun getValue(owner: Any, p: KProperty<*>): T = state.value
        operator fun setValue(owner: Any, p: KProperty<*>, value: T) {
            state.value = value
            prefs.edit().putString(key, write(value)).apply()
        }
    }

    private fun <E : Enum<E>> enum(key: String, values: Array<E>, default: E) =
        Saved(key, { s -> values.firstOrNull { it.name == s } ?: default }, { it.name })

    private fun bool(key: String, default: Boolean) =
        Saved(key, { s -> s?.toBooleanStrictOrNull() ?: default }, { it.toString() })

    var motion by enum("motion", MotionSetting.entries.toTypedArray(), MotionSetting.AUTOMATIC)
    var tempoLookup by bool("tempoLookup", true)
    var artMode by enum("artMode", ArtMode.entries.toTypedArray(), ArtMode.PALETTE)
    var artDetail by enum("artDetail", ArtDetail.entries.toTypedArray(), ArtDetail.FINE)

    /** Lyrics instead of the cover (the cover keeps moving behind them). */
    var showLyrics by bool("showLyrics", false)
    var lyricStyle by enum("lyricStyle", LyricStyleKind.entries.toTypedArray(), LyricStyleKind.BIG_ONLY)
    var lyricScroll by enum("lyricScroll", LyricScrollKind.entries.toTypedArray(), LyricScrollKind.UP)
    var lyricPaint by enum("lyricPaint", LyricPaintKind.entries.toTypedArray(), LyricPaintKind.ACCENT)
    var lyricSize by enum("lyricSize", LyricSizeKind.entries.toTypedArray(), LyricSizeKind.MEDIUM)
    var gestures by enum("gestures", GestureScheme.entries.toTypedArray(), GestureScheme.DIRECT)

    /** The gesture guide has been shown once. */
    var seenGestureGuide by bool("seenGestureGuide", false)
    var saveAhead by enum("saveAhead", SaveScope.entries.toTypedArray(), SaveScope.LIKED)

    /** Save ahead over mobile data too (else Wi-Fi only). */
    var saveOnMobileData by bool("saveOnMobileData", false)

    /** The source to reconnect to on launch. */
    var lastSource by Saved("lastSource", { SourceKind.from(it) }, { it?.key })

    /** A Spotify client ID entered in Settings (used when none is built in). */
    var spotifyClientID by Saved("spotifyClientID", { it ?: "" }, { it })

    fun animates(reduceMotion: Boolean) = when (motion) {
        MotionSetting.AUTOMATIC -> !reduceMotion
        MotionSetting.ON -> true
        MotionSetting.OFF -> false
    }
}
