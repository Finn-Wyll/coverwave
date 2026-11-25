package app.coverwave.core

import android.media.MediaCodec
import android.media.MediaExtractor
import android.media.MediaFormat
import app.coverwave.sources.NowPlaying
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import uniffi.cw_ffi.LyricSheet
import uniffi.cw_ffi.Rhythm
import uniffi.cw_ffi.RhythmEngine
import uniffi.cw_ffi.parseLyrics
import java.io.File
import java.nio.ByteOrder

/**
 * A song on Deezer, found once per track (by ISRC, else artist and title)
 * and shared by the rhythm map and the genre lookup. Sends only the ISRC,
 * or the artist and title.
 */
object Deezer {
    class Track(val id: Long?, val preview: String?, val duration: Int?, val albumID: Long?)

    private val found = HashMap<String, CompletableDeferred<Track?>>()

    suspend fun track(np: NowPlaying): Track? {
        val existing = synchronized(found) { found[np.trackID] }
        if (existing != null) return existing.await()
        val d = CompletableDeferred<Track?>()
        synchronized(found) { found[np.trackID] = d }
        val t = runCatching { find(np) }.getOrNull()
        d.complete(t)
        return t
    }

    private fun parse(o: JSONObject?): Track? {
        if (o == null || o.has("error") || !o.has("id")) return null
        return Track(
            id = o.optLong("id"),
            preview = o.str("preview")?.takeIf { it.isNotEmpty() },
            duration = if (o.has("duration")) o.optInt("duration") else null,
            albumID = o.obj("album")?.optLong("id"),
        )
    }

    /** The genres of the song's album on Deezer ("Rock, Alternative"). */
    suspend fun genre(np: NowPlaying): String? {
        val id = track(np)?.albumID ?: return null
        val album = Http.json("https://api.deezer.com/album/$id") ?: return null
        val names = album.obj("genres")?.arr("data")?.objects()?.mapNotNull { it.str("name") }
            ?.filter { it.isNotEmpty() }.orEmpty()
        return names.takeIf { it.isNotEmpty() }?.joinToString(", ")
    }

    fun mainArtist(np: NowPlaying) = np.artist.split(",").first().trim()

    private suspend fun find(np: NowPlaying): Track? {
        np.isrc?.let { isrc ->
            parse(Http.json("https://api.deezer.com/track/isrc:$isrc"))?.let { return it }
        }
        val artist = mainArtist(np).replace("\"", "")
        if (artist.isEmpty()) return null
        val q = "artist:\"$artist\" track:\"${np.title.replace("\"", "")}\""
        val search = Http.json("https://api.deezer.com/search", listOf("limit" to "1", "q" to q)) ?: return null
        val id = search.arr("data")?.objects()?.firstOrNull()?.optLong("id") ?: return null
        return parse(Http.json("https://api.deezer.com/track/$id"))
    }
}

/** A genre for songs whose source gave none: the album's genres on Deezer. */
object GenreLookup {
    private val found = HashMap<String, Deferred<String?>>()
    private val scope = kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.SupervisorJob() + Dispatchers.IO)

    suspend fun genre(np: NowPlaying): String? {
        val d = synchronized(found) {
            found.getOrPut(np.trackID) { scope.async { runCatching { Deezer.genre(np) }.getOrNull() } }
        }
        return d.await()
    }
}

enum class LyricsStatus { SEARCHING, FOUND, INSTRUMENTAL, NOT_FOUND }

/**
 * Lyrics from LRCLIB (lrclib.net), a free, keyless database of time-synced
 * lyrics, like the other apps. Sends the song's artist, title, album and
 * length. Kept with the offline data, so a song is looked up once (misses
 * again after a week) and its lyrics are there offline.
 */
object LyricsLookup {
    private const val INSTRUMENTAL = "[instrumental]"
    private const val MISS_TTL_MS = 7L * 24 * 3600 * 1000

    private fun file(trackID: String) = File(OfflineStore.folder, "lyrics/${hashed(trackID)}.lrc")

    fun isSaved(trackID: String) = file(trackID).exists()

    /** Without [network], only what was saved. */
    suspend fun lyrics(np: NowPlaying, network: Boolean = true): Pair<LyricSheet?, LyricsStatus> =
        withContext(Dispatchers.IO) {
            val f = file(np.trackID)
            if (f.exists()) {
                val text = f.readText()
                val age = System.currentTimeMillis() - f.lastModified()
                when {
                    text.isBlank() -> if (age < MISS_TTL_MS) return@withContext null to LyricsStatus.NOT_FOUND
                    text == INSTRUMENTAL -> return@withContext null to LyricsStatus.INSTRUMENTAL
                    else -> parseLyrics(text)?.let { return@withContext it to LyricsStatus.FOUND }
                }
            }
            if (!network) return@withContext null to LyricsStatus.NOT_FOUND
            when (val found = fetch(np)) {
                Fetch.Unreachable -> null to LyricsStatus.NOT_FOUND
                Fetch.Missing -> {
                    f.writeAtomically(ByteArray(0))
                    null to LyricsStatus.NOT_FOUND
                }
                Fetch.Instrumental -> {
                    f.writeAtomically(INSTRUMENTAL.toByteArray())
                    null to LyricsStatus.INSTRUMENTAL
                }
                is Fetch.Text -> {
                    f.writeAtomically(found.text.toByteArray())
                    val sheet = parseLyrics(found.text)
                    sheet to (if (sheet != null) LyricsStatus.FOUND else LyricsStatus.NOT_FOUND)
                }
            }
        }

    private sealed class Fetch {
        data object Unreachable : Fetch()
        data object Missing : Fetch()
        data object Instrumental : Fetch()
        data class Text(val text: String) : Fetch()
    }

    private fun pick(o: JSONObject): Fetch? {
        if (o.optBoolean("instrumental")) return Fetch.Instrumental
        for (k in listOf("syncedLyrics", "plainLyrics")) {
            val t = o.str(k)
            if (!t.isNullOrBlank()) return Fetch.Text(t)
        }
        return null
    }

    private suspend fun fetch(np: NowPlaying): Fetch {
        val artist = Deezer.mainArtist(np)
        if (artist.isEmpty() || np.title.isEmpty()) return Fetch.Missing
        val query = mutableListOf("artist_name" to artist, "track_name" to np.title)
        if (np.album.isNotEmpty()) query += "album_name" to np.album
        np.duration?.let { query += "duration" to Math.round(it).toString() }
        val first = runCatching { Http.request(Http.withQuery("https://lrclib.net/api/get", query)) }
            .getOrElse { return Fetch.Unreachable }
        if (first.code == 200) pick(first.json())?.let { return it }
        val second = runCatching { Http.request(Http.withQuery("https://lrclib.net/api/search", query.take(2))) }
            .getOrElse { return Fetch.Unreachable }
        if (second.code != 200) return Fetch.Missing
        val records = runCatching { JSONArray(second.text).objects() }.getOrDefault(emptyList())
        // The closest synced version within 10s of the song's length.
        fun off(r: JSONObject): Double {
            val a = if (r.has("duration")) r.optDouble("duration") else return 0.0
            val b = np.duration ?: return 0.0
            return kotlin.math.abs(a - b)
        }
        val close = records.filter { off(it) <= 10 }
        val synced = close.filter { !it.str("syncedLyrics").isNullOrBlank() }.minByOrNull { off(it) }
        return (synced ?: close.firstOrNull { pick(it) != null })?.let { pick(it) } ?: Fetch.Missing
    }
}

/**
 * A song's rhythm map: its 30-second Deezer preview, run through the Beat
 * This! beat tracker in the Rust core, gives the tempo, metre, bar phase and
 * the song's own groove. Maps are cached per track, so a song is measured
 * once.
 */
object RhythmLookup {
    /**
     * Deezer cuts every preview from 48 seconds into the track, so the
     * preview's beat grid extends to the whole song.
     */
    private const val DEEZER_PREVIEW_START = 48.0

    private var engine: RhythmEngine? = null
    private var engineTried = false

    /** The bundled model (`assets/BeatThis-small.onnx`), loaded once. */
    @Synchronized
    private fun engine(): RhythmEngine? {
        if (engineTried) return engine
        engineTried = true
        engine = runCatching {
            val bytes = Platform.context.assets.open("BeatThis-small.onnx").use { it.readBytes() }
            RhythmEngine(bytes)
        }.getOrNull()
        return engine
    }

    private fun file(trackID: String) = File(OfflineStore.folder, "rhythm/${hashed(trackID)}.json")

    /** Without [network], only a saved map. */
    suspend fun rhythm(np: NowPlaying, network: Boolean = true): Rhythm? {
        load(np.trackID)?.let { return it }
        if (!network) return null
        val deezer = Deezer.track(np) ?: return null
        val preview = deezer.preview ?: return null
        val data = Http.bytes(preview) ?: return null
        val start = clipStart(deezer, np)
        val rhythm = withContext(Dispatchers.Default) {
            val e = engine() ?: return@withContext fallback(data)
            val (samples, rate) = decodeMono(data) ?: return@withContext null
            runCatching { e.measure(samples.asList(), rate.toUInt(), start) }.getOrNull()
        } ?: return null
        withContext(Dispatchers.IO) { file(np.trackID).writeAtomically(rhythm.toJson().toByteArray()) }
        return rhythm
    }

    /** Without the beat model nothing is measured (the genre's tempo is used). */
    private fun fallback(@Suppress("UNUSED_PARAMETER") data: ByteArray): Rhythm? = null

    private fun load(trackID: String): Rhythm? {
        val f = file(trackID)
        if (!f.exists()) return null
        return runCatching { Rhythm.fromJson(f.readText()) }.getOrNull()
    }

    /** Where the preview starts in the song playing, if that is known. */
    private fun clipStart(d: Deezer.Track, playing: NowPlaying): Double? {
        val dur = d.duration ?: return null
        if (dur < DEEZER_PREVIEW_START + 30) return null
        playing.duration?.let { if (kotlin.math.abs(it - dur) > 3) return null }
        return DEEZER_PREVIEW_START
    }

    /** Decodes a preview (MP3) with MediaCodec and mixes it to mono. */
    fun decodeMono(encoded: ByteArray): Pair<FloatArray, Int>? {
        val tmp = File.createTempFile("coverwave-preview", ".mp3", Platform.cacheDir)
        try {
            tmp.writeBytes(encoded)
            val ex = MediaExtractor()
            ex.setDataSource(tmp.absolutePath)
            val track = (0 until ex.trackCount).firstOrNull {
                ex.getTrackFormat(it).getString(MediaFormat.KEY_MIME)?.startsWith("audio/") == true
            } ?: return null
            ex.selectTrack(track)
            val format = ex.getTrackFormat(track)
            val codec = MediaCodec.createDecoderByType(format.getString(MediaFormat.KEY_MIME)!!)
            codec.configure(format, null, null, 0)
            codec.start()
            var rate = format.getInteger(MediaFormat.KEY_SAMPLE_RATE)
            var channels = format.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
            val out = ArrayList<Float>(rate * 32)
            val info = MediaCodec.BufferInfo()
            var inputDone = false
            var outputDone = false
            while (!outputDone) {
                if (!inputDone) {
                    val i = codec.dequeueInputBuffer(10_000)
                    if (i >= 0) {
                        val buf = codec.getInputBuffer(i)!!
                        val n = ex.readSampleData(buf, 0)
                        if (n < 0) {
                            codec.queueInputBuffer(i, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                            inputDone = true
                        } else {
                            codec.queueInputBuffer(i, 0, n, ex.sampleTime, 0)
                            ex.advance()
                        }
                    }
                }
                val o = codec.dequeueOutputBuffer(info, 10_000)
                if (o == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                    rate = codec.outputFormat.getInteger(MediaFormat.KEY_SAMPLE_RATE)
                    channels = codec.outputFormat.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
                } else if (o >= 0) {
                    val buf = codec.getOutputBuffer(o)!!.order(ByteOrder.LITTLE_ENDIAN)
                    buf.position(info.offset)
                    buf.limit(info.offset + info.size)
                    val shorts = buf.asShortBuffer()
                    val frames = shorts.remaining() / channels
                    for (f in 0 until frames) {
                        var s = 0f
                        for (c in 0 until channels) s += shorts.get(f * channels + c) / 32768f
                        out.add(s / channels)
                    }
                    codec.releaseOutputBuffer(o, false)
                    if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) outputDone = true
                }
            }
            codec.stop()
            codec.release()
            ex.release()
            return if (out.isEmpty()) null else out.toFloatArray() to rate
        } catch (_: Exception) {
            return null
        } finally {
            tmp.delete()
        }
    }
}
