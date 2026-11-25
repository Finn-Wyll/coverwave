package app.coverwave.library

import app.coverwave.core.Http
import app.coverwave.core.OfflineStore
import app.coverwave.core.OfflineStore.Row
import app.coverwave.core.arr
import app.coverwave.core.isOffline
import app.coverwave.core.obj
import app.coverwave.core.objects
import app.coverwave.core.str
import app.coverwave.core.timeText
import app.coverwave.sources.MediaSessionSource
import app.coverwave.sources.SpotifySource
import org.json.JSONObject
import java.util.UUID

/**
 * The Spotify library through the Web API, with [SpotifySource]'s login.
 * Mirrors the terminal and iPhone apps, including Spotify's February 2026
 * Development Mode rules: playlist entries come from `/playlists/{id}/items`
 * (under `item`, or `track` before), and only playlists you own or
 * collaborate on can be listed.
 *
 * Every listing is saved ([OfflineStore]), with its tracks' details, so the
 * library still opens offline; rows then play through the Spotify app.
 */
class SpotifyLibrary(private val source: SpotifySource) : LibraryProvider {
    override val name = "spotify"

    /** Spotify's queue only grows at the end. */
    override val queuePositions = listOf(QueuePosition.LAST)

    private var me: String? = null
    private val maxRows = 1000

    override fun reauthorize() {
        me = null
        source.reauthorize()
    }

    override suspend fun list(path: LibraryPath): List<LibraryItem> = rows(path).map(::item)

    /** A listing's rows: from Spotify (and saved), or the saved copy offline. */
    suspend fun rows(path: LibraryPath): List<Row> {
        if (path.id == "root") return root
        if (source.followingApp) return saved(path)
        return try {
            fetch(path).also { OfflineStore.saveListing(path.id, it) }
        } catch (e: Exception) {
            if (e.isOffline) saved(path) else throw e
        }
    }

    private fun saved(path: LibraryPath): List<Row> = OfflineStore.listing(path.id)
        ?: throw LibraryException.Message(
            "Not saved for offline yet. Open it once while online, or turn on Save Ahead in Settings.",
        )

    private val root = listOf(
        Row(id = "liked", title = "liked songs", opensID = "liked", opensTitle = "liked songs"),
        Row(id = "playlists", title = "playlists", opensID = "playlists", opensTitle = "playlists"),
        Row(id = "albums", title = "albums", opensID = "albums", opensTitle = "albums"),
    )

    private suspend fun fetch(path: LibraryPath): List<Row> = when (path.id) {
        "liked" -> {
            val raw = all("/me/tracks").mapNotNull { it.obj("track") }
            remember(raw)
            val tracks = raw.mapNotNull(::track)
            val uris = tracks.map { it.uri }
            listOf(playAll(tracks.size, Row.Action.Tracks(uris))) +
                tracks.mapIndexed { i, t -> row(t, Row.Action.Tracks(uris.drop(i))) }
        }
        "playlists" -> {
            val mine = userID()
            all("/me/playlists").mapNotNull { playlistRow(it, mine) }
        }
        "albums" -> all("/me/albums").mapNotNull { e ->
            val a = e.obj("album") ?: return@mapNotNull null
            val id = a.str("id") ?: return@mapNotNull null
            val uri = a.str("uri") ?: return@mapNotNull null
            val n = a.str("name") ?: "Untitled"
            Row(
                id = "album:$id", title = n, subtitle = artists(a),
                detail = if (a.has("total_tracks")) a.optInt("total_tracks").toString() else null,
                opensID = "album:$id:$uri", opensTitle = n,
            )
        }
        else -> {
            // "playlist:ID:spotify:playlist:ID" or "album:ID:spotify:album:ID".
            val parts = path.id.split(":", limit = 3)
            if (parts.size != 3) emptyList()
            else {
                val (kind, id, uri) = parts
                val raw = if (kind == "playlist") {
                    all("/playlists/$id/items").mapNotNull { it.obj("item") ?: it.obj("track") }
                } else {
                    all("/albums/$id/tracks")
                }
                if (kind == "playlist") remember(raw)
                val tracks = raw.mapNotNull(::track)
                listOf(playAll(tracks.size, Row.Action.Context(uri, null, null))) +
                    tracks.mapIndexed { i, t -> row(t, Row.Action.Context(uri, t.uri, i)) }
            }
        }
    }

    override suspend fun search(term: String): List<LibraryItem> {
        if (source.followingApp) return savedSearch(term)
        val json = try {
            get("/search?q=${Http.enc(term)}&type=track,album,playlist&limit=10")
        } catch (e: Exception) {
            if (e.isOffline) return savedSearch(term) else throw e
        }
        fun items(key: String) = json.obj(key)?.arr("items")?.objects().orEmpty()
        val playlists = items("playlists").mapNotNull { p ->
            val uri = p.str("uri") ?: return@mapNotNull null
            Row(id = uri, title = p.str("name") ?: "Untitled", subtitle = "playlist", action = Row.Action.Context(uri, null, null))
        }
        val albums = items("albums").mapNotNull { a ->
            val id = a.str("id") ?: return@mapNotNull null
            val uri = a.str("uri") ?: return@mapNotNull null
            val n = a.str("name") ?: "Untitled"
            Row(id = uri, title = n, subtitle = artists(a), opensID = "album:$id:$uri", opensTitle = n)
        }
        val tracks = items("tracks").mapNotNull(::track).map { row(it, Row.Action.Tracks(listOf(it.uri))) }
        return (tracks + albums + playlists).map(::item)
    }

    /** Offline search: the saved songs. */
    private fun savedSearch(term: String) = OfflineStore.search(term).map { r ->
        item(
            Row(
                id = r.id, title = r.title, subtitle = r.artist, detail = r.duration?.let { timeText(it) },
                action = Row.Action.Tracks(listOf(r.id)), trackURI = r.id,
            ),
        )
    }

    override suspend fun upNext(): QueueSnapshot {
        if (source.followingApp) {
            val current = source.nowPlaying?.let { QueueSnapshot.Entry(it.trackID, it.title, it.artist) }
            return QueueSnapshot(current, emptyList(), "Spotify's queue can't be read offline.")
        }
        val json = get("/me/player/queue")
        fun entry(o: JSONObject?) = track(o)?.let { QueueSnapshot.Entry(UUID.randomUUID().toString(), it.name, it.artists) }
        val upcoming = json.arr("queue")?.objects().orEmpty().take(50).mapNotNull(::entry)
        return QueueSnapshot(entry(json.obj("currently_playing")), upcoming)
    }

    // Rows

    private fun playlistRow(p: JSONObject, mine: String): Row? {
        val id = p.str("id") ?: return null
        val uri = p.str("uri") ?: return null
        val n = p.str("name") ?: "Untitled"
        val ownerInfo = p.obj("owner") ?: JSONObject()
        val owner = ownerInfo.str("id") ?: ""
        val ownerName = ownerInfo.str("display_name") ?: owner
        val readable = owner == mine || p.optBoolean("collaborative")
        val total = (p.obj("items") ?: p.obj("tracks"))?.takeIf { it.has("total") }?.optInt("total")
        val row = Row(
            id = "playlist:$id", title = n, subtitle = if (owner == mine) null else "by $ownerName",
            detail = total?.toString(),
        )
        if (readable) {
            row.opensID = "playlist:$id:$uri"
            row.opensTitle = n
        } else {
            // Others' playlists can be played but not listed.
            row.action = Row.Action.Context(uri, null, null)
        }
        return row
    }

    private data class Track(val uri: String, val name: String, val artists: String?, val durationMS: Int?)

    private fun track(t: JSONObject?): Track? {
        if (t == null || t.optBoolean("is_local")) return null
        val uri = t.str("uri") ?: return null
        return Track(
            uri, t.str("name") ?: "Untitled", artists(t) ?: t.obj("show")?.str("name"),
            if (t.has("duration_ms")) t.optInt("duration_ms") else null,
        )
    }

    /** Saves the tracks' details (cover URL, ISRC) for offline. */
    private fun remember(raw: List<JSONObject>) {
        for (t in raw) {
            if (t.optBoolean("is_local")) continue
            val uri = t.str("uri")?.takeIf { it.startsWith("spotify:track:") } ?: continue
            val album = t.obj("album") ?: JSONObject()
            val cover = album.arr("images")?.objects()?.maxByOrNull { it.optInt("width") }
            OfflineStore.remember(
                OfflineStore.Record(
                    id = uri, title = t.str("name") ?: "Untitled", artist = artists(t) ?: "",
                    album = album.str("name") ?: "",
                    duration = if (t.has("duration_ms")) t.optDouble("duration_ms") / 1000 else null,
                    isrc = t.obj("external_ids")?.str("isrc"), artworkURL = cover?.str("url"),
                ),
            )
        }
    }

    private fun artists(v: JSONObject): String? =
        v.arr("artists")?.objects()?.mapNotNull { it.str("name") }?.takeIf { it.isNotEmpty() }?.joinToString(", ")

    private fun row(t: Track, action: Row.Action) = Row(
        id = UUID.randomUUID().toString(), title = t.name, subtitle = t.artists,
        detail = t.durationMS?.let { timeText(it / 1000.0) }, action = action, trackURI = t.uri,
    )

    private fun playAll(n: Int, action: Row.Action) = Row(id = "play-all", title = "play all", detail = "$n", action = action)

    /** A saved row with its actions. */
    private fun item(r: Row): LibraryItem {
        val play: (suspend () -> Unit)? = when (val a = r.action) {
            is Row.Action.Context -> { { source.play(a.uri, a.offset, a.index) } }
            is Row.Action.Tracks -> { { source.play(a.uris) } }
            null -> null
        }
        val enqueue: (suspend (QueuePosition) -> Unit)? = r.trackURI?.let { uri -> { _ -> source.enqueue(uri) } }
        return LibraryItem(
            id = r.id, title = r.title, subtitle = r.subtitle, detail = r.detail,
            opens = r.opensID?.let { LibraryPath(it, r.opensTitle ?: r.title) },
            play = play, enqueue = enqueue,
        )
    }

    // HTTP

    private suspend fun userID(): String {
        me?.let { return it }
        val id = get("/me").str("id") ?: ""
        me = id
        return id
    }

    /** Every page of a paged listing, up to [maxRows]. */
    private suspend fun all(path: String): List<JSONObject> {
        val out = mutableListOf<JSONObject>()
        var offset = 0
        while (out.size < maxRows) {
            val sep = if (path.contains("?")) "&" else "?"
            val page = get("$path${sep}limit=50&offset=$offset")
            val items = page.arr("items")?.objects().orEmpty()
            out += items
            offset += 50
            if (items.isEmpty() || page.isNull("next")) break
        }
        return out
    }

    private suspend fun get(path: String): JSONObject {
        val r = source.call("GET", path)
        // 403 on reads: the login lacks the library scopes.
        if (r.code == 403) throw LibraryException.NeedsLogin()
        if (r.code !in 200..299) throw LibraryException.Message("Spotify said ${r.code}.")
        return if (r.body.isEmpty()) JSONObject() else r.json()
    }
}

/**
 * The library panel while following a media session: Android does not let
 * another app browse Apple Music's library, so it says so; the queue is the
 * player's own.
 */
class MediaLibrary(private val source: MediaSessionSource) : LibraryProvider {
    override val name = "apple music"
    override val queuePositions = emptyList<QueuePosition>()

    override suspend fun list(path: LibraryPath): List<LibraryItem> = throw LibraryException.Message(
        "Android doesn't let other apps browse ${source.playerName ?: "Apple Music"}'s library. Pick songs there; coverwave follows along.",
    )

    override suspend fun search(term: String): List<LibraryItem> = emptyList()

    override suspend fun upNext(): QueueSnapshot {
        val current = source.nowPlaying?.let { QueueSnapshot.Entry(it.trackID, it.title, it.artist) }
        val queue = source.queue()
        val upcoming = queue.orEmpty().mapIndexed { i, (t, s) -> QueueSnapshot.Entry("q$i", t, s) }
            .dropWhile { it.title == current?.title }
        return QueueSnapshot(
            current, upcoming,
            if (queue == null) "${source.playerName ?: "This player"} doesn't share its queue." else null,
        )
    }

    override fun reauthorize() {}
}
