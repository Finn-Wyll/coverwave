package app.coverwave.core

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import app.coverwave.sources.NowPlaying
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * What coverwave keeps for listening without the internet, like the other
 * apps: per Spotify track its details and cover, and the library listings
 * last seen or saved ahead. Lyrics and rhythm maps live next to it.
 * Spotify's downloaded songs stay inside the Spotify app.
 */
object OfflineStore {
    val folder: File get() = File(Platform.filesDir, "offline")

    data class Record(
        /** Spotify URI (as `NowPlaying.trackID`). */
        val id: String,
        val title: String,
        val artist: String,
        val album: String,
        val duration: Double? = null,
        val isrc: String? = null,
        val artworkURL: String? = null,
        val genre: String? = null,
        val genreChecked: Boolean = false,
        val lyricsChecked: Boolean = false,
        val rhythmChecked: Boolean = false,
        val coverChecked: Boolean = false,
    ) {
        val nowPlaying: NowPlaying
            get() = NowPlaying(
                trackID = id, title = title, artist = artist, album = album, duration = duration,
                position = 0.0, readAt = 0, isPlaying = false, genre = genre, isrc = isrc,
                artworkURL = artworkURL,
            )

        fun toJson(): JSONObject = JSONObject().apply {
            put("id", id); put("title", title); put("artist", artist); put("album", album)
            duration?.let { put("duration", it) }
            isrc?.let { put("isrc", it) }
            artworkURL?.let { put("artworkURL", it) }
            genre?.let { put("genre", it) }
            put("genreChecked", genreChecked); put("lyricsChecked", lyricsChecked)
            put("rhythmChecked", rhythmChecked); put("coverChecked", coverChecked)
        }

        companion object {
            fun from(o: JSONObject) = Record(
                id = o.getString("id"), title = o.optString("title"), artist = o.optString("artist"),
                album = o.optString("album"),
                duration = if (o.has("duration")) o.optDouble("duration") else null,
                isrc = o.str("isrc"), artworkURL = o.str("artworkURL"), genre = o.str("genre"),
                genreChecked = o.optBoolean("genreChecked"), lyricsChecked = o.optBoolean("lyricsChecked"),
                rhythmChecked = o.optBoolean("rhythmChecked"), coverChecked = o.optBoolean("coverChecked"),
            )
        }
    }

    /** A listing row without its actions, so it can be saved. */
    data class Row(
        val id: String,
        val title: String,
        val subtitle: String? = null,
        val detail: String? = null,
        var opensID: String? = null,
        var opensTitle: String? = null,
        var action: Action? = null,
        /** The track to queue, if it can be queued. */
        val trackURI: String? = null,
    ) {
        sealed class Action {
            /** Play a context (album or playlist), from one of its tracks. */
            data class Context(val uri: String, val offset: String?, val index: Int?) : Action()

            /** Play these tracks in order. */
            data class Tracks(val uris: List<String>) : Action()
        }

        fun toJson(): JSONObject = JSONObject().apply {
            put("id", id); put("title", title)
            subtitle?.let { put("subtitle", it) }
            detail?.let { put("detail", it) }
            opensID?.let { put("opensID", it) }
            opensTitle?.let { put("opensTitle", it) }
            trackURI?.let { put("trackURI", it) }
            when (val a = action) {
                is Action.Context -> put("context", JSONObject().apply {
                    put("uri", a.uri); a.offset?.let { put("offset", it) }; a.index?.let { put("index", it) }
                })
                is Action.Tracks -> put("tracks", JSONArray(a.uris))
                null -> {}
            }
        }

        companion object {
            fun from(o: JSONObject): Row {
                val action = o.obj("context")?.let {
                    Action.Context(it.getString("uri"), it.str("offset"), if (it.has("index")) it.optInt("index") else null)
                } ?: o.arr("tracks")?.let { a -> Action.Tracks((0 until a.length()).map { a.getString(it) }) }
                return Row(
                    id = o.getString("id"), title = o.optString("title"), subtitle = o.str("subtitle"),
                    detail = o.str("detail"), opensID = o.str("opensID"), opensTitle = o.str("opensTitle"),
                    action = action, trackURI = o.str("trackURI"),
                )
            }
        }
    }

    private val records = HashMap<String, Record>()
    private var loaded = false

    /** Bumped on every change, so views showing counts refresh. */
    var revision by mutableIntStateOf(0)
        private set

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var saveJob: Job? = null

    @Synchronized
    private fun ensureLoaded() {
        if (loaded) return
        loaded = true
        runCatching {
            val o = JSONObject(File(folder, "tracks.json").readText())
            for (k in o.keys()) records[k] = Record.from(o.getJSONObject(k))
        }
    }

    @Synchronized
    fun all(): List<Record> {
        ensureLoaded()
        return records.values.toList()
    }

    @Synchronized
    fun record(id: String): Record? {
        ensureLoaded()
        return records[id]
    }

    /** Adds or updates a track, keeping what is already known. */
    @Synchronized
    fun remember(r: Record) {
        ensureLoaded()
        val old = records[r.id]
        val merged = if (old == null) r else old.copy(
            title = r.title.ifEmpty { old.title },
            artist = r.artist.ifEmpty { old.artist },
            album = r.album.ifEmpty { old.album },
            duration = r.duration ?: old.duration,
            isrc = r.isrc ?: old.isrc,
            artworkURL = r.artworkURL ?: old.artworkURL,
            genre = r.genre ?: old.genre,
            genreChecked = old.genreChecked || r.genreChecked,
            lyricsChecked = old.lyricsChecked || r.lyricsChecked,
            rhythmChecked = old.rhythmChecked || r.rhythmChecked,
            coverChecked = old.coverChecked || r.coverChecked,
        )
        if (old == merged) return
        records[r.id] = merged
        changed()
    }

    /** Remembers what a source reports for the playing Spotify track. */
    fun remember(np: NowPlaying) {
        if (!np.trackID.startsWith("spotify:")) return
        remember(
            Record(
                id = np.trackID, title = np.title, artist = np.artist, album = np.album,
                duration = np.duration, isrc = np.isrc, artworkURL = np.artworkURL,
                genre = np.genre, genreChecked = np.genre != null,
            ),
        )
    }

    @Synchronized
    fun update(id: String, change: (Record) -> Record) {
        ensureLoaded()
        val r = records[id] ?: return
        val n = change(r)
        if (n == r) return
        records[id] = n
        changed()
    }

    /** Saved tracks matching every word of [term]. */
    fun search(term: String, limit: Int = 50): List<Record> {
        val words = term.lowercase().split(" ").filter { it.isNotEmpty() }
        if (words.isEmpty()) return emptyList()
        return all().filter { r ->
            val hay = "${r.title} ${r.artist} ${r.album}".lowercase()
            words.all { hay.contains(it) }
        }.sortedBy { it.title.lowercase() }.take(limit)
    }

    /** Fills in what a source left out (genre, ISRC, cover URL). */
    fun enrich(np: NowPlaying): NowPlaying {
        val r = record(np.trackID) ?: return np
        return np.copy(
            genre = np.genre ?: r.genre, isrc = np.isrc ?: r.isrc,
            artworkURL = np.artworkURL ?: r.artworkURL,
        )
    }

    // Covers

    /** The saved cover: downloaded from its URL (same bytes as online), else the app's copy. */
    fun cover(np: NowPlaying): ByteArray? {
        val url = np.artworkURL ?: record(np.trackID)?.artworkURL
        if (url != null) coverFile(url).takeIf { it.exists() }?.let { return it.readBytes() }
        return coverFile(np.trackID).takeIf { it.exists() }?.readBytes()
    }

    fun hasCover(url: String) = coverFile(url).exists()

    fun saveCover(data: ByteArray, key: String) = coverFile(key).writeAtomically(data)

    private fun coverFile(key: String) = File(folder, "covers/${hashed(key)}.img")

    // Listings

    fun saveListing(pathID: String, rows: List<Row>) {
        listingFile(pathID).writeAtomically(JSONArray(rows.map { it.toJson() }).toString().toByteArray())
    }

    fun listing(pathID: String): List<Row>? = runCatching {
        JSONArray(listingFile(pathID).readText()).objects().map { Row.from(it) }
    }.getOrNull()

    private fun listingFile(pathID: String) = File(folder, "library/${hashed(pathID)}.json")

    /** Bytes on disk: this store plus saved lyrics and rhythm maps. */
    fun diskBytes(): Long = folder.walkTopDown().filter { it.isFile }.sumOf { it.length() }

    /** Forgets everything saved for offline. */
    @Synchronized
    fun clear() {
        saveJob?.cancel()
        folder.deleteRecursively()
        records.clear()
        Platform.onMain { revision++ }
    }

    /** Writes the track index a moment after the last change. */
    private fun changed() {
        Platform.onMain { revision++ }
        saveJob?.cancel()
        saveJob = scope.launch {
            delay(1000)
            val json = synchronized(this@OfflineStore) {
                JSONObject().apply { records.forEach { (k, v) -> put(k, v.toJson()) } }.toString()
            }
            File(folder, "tracks.json").writeAtomically(json.toByteArray())
        }
    }
}
