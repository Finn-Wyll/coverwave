package app.coverwave.sources

/**
 * Where the music plays. coverwave never plays audio itself on Android: it
 * follows and controls another app (as on iPhone, `ios/Coverwave/Sources`).
 */
enum class SourceKind(val key: String, val label: String, val blurb: String) {
    SPOTIFY("spotify", "Spotify", "follows spotify on any device"),

    /** Apple Music, or any other player, through Android's media sessions. */
    APPLE_MUSIC("appleMusic", "Apple Music", "follows apple music, or any player"),

    /**
     * Follows the Spotify app on this phone directly (its downloads play
     * without data) and uses only what was saved ahead.
     */
    OFFLINE("offline", "Offline", "spotify downloads, saved data only");

    companion object {
        fun from(key: String?): SourceKind? = entries.firstOrNull { it.key == key }
    }
}

/**
 * The track a source is playing, as last read. The position is
 * extrapolated between reads with [positionAt].
 */
data class NowPlaying(
    /** Stable per track (a Spotify URI, or `media:<player>:<id>`). */
    val trackID: String,
    val title: String,
    val artist: String,
    val album: String,
    /** Seconds. */
    val duration: Double?,
    val position: Double,
    /** `System.currentTimeMillis()` when the position was read. */
    val readAt: Long,
    val isPlaying: Boolean,
    /** Raw genre tag(s), comma-separated; the Rust core maps them to a family. */
    val genre: String? = null,
    /** For the rhythm lookup. */
    val isrc: String? = null,
    /**
     * Cover URL whose bytes match what the terminal and iPhone apps download,
     * so all of them give a cover the same seed and motion.
     */
    val artworkURL: String? = null,
) {
    fun positionAt(nowMs: Long = System.currentTimeMillis()): Double {
        if (!isPlaying) return position
        val p = position + maxOf(0L, nowMs - readAt) / 1000.0
        return duration?.let { minOf(p, it) } ?: p
    }

    /**
     * The same track and state, with the position where it was expected to
     * be. Sources use this to skip reporting reads that change nothing.
     */
    fun matches(other: NowPlaying): Boolean {
        val drift = kotlin.math.abs(positionAt(other.readAt) - other.position)
        return copy(position = 0.0, readAt = 0) == other.copy(position = 0.0, readAt = 0) && drift < 0.5
    }
}

sealed class SourceStatus {
    data object Disconnected : SourceStatus()
    data object Connecting : SourceStatus()
    data object Connected : SourceStatus()

    /** The user has to approve access first (a tap on Connect). */
    data object NeedsAuthorization : SourceStatus()
    data class Unavailable(val text: String) : SourceStatus()
    data class Failed(val text: String) : SourceStatus()

    val message: String?
        get() = when (this) {
            is Unavailable -> text
            is Failed -> text
            NeedsAuthorization -> "Needs your permission"
            Connecting -> "Connecting…"
            else -> null
        }
}

/** Everything a source does, on the main thread. */
interface MusicSource {
    val kind: SourceKind
    val status: SourceStatus
    val nowPlaying: NowPlaying?

    /** Called on the main thread whenever [nowPlaying] changes. */
    var onChange: ((NowPlaying?) -> Unit)?

    /** Starts following. May ask the user for permission (call from a tap). */
    fun connect()

    /** Reconnects quietly if the user already gave permission. */
    fun resume()
    fun disconnect()

    /** The cover image's bytes (JPEG or PNG), or null. */
    suspend fun artwork(track: NowPlaying): ByteArray?

    fun playPause()
    fun next()
    fun previous()
    fun seek(seconds: Double)
}
