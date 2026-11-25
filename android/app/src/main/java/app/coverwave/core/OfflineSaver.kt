package app.coverwave.core

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import app.coverwave.AppModel
import app.coverwave.SaveScope
import app.coverwave.library.LibraryPath
import app.coverwave.sources.SourceKind
import app.coverwave.sources.SourceStatus
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Saves ahead, while online, what Spotify songs need to play offline with
 * their own motion, like the other apps: the liked songs' (and playlists')
 * listings, then per song its cover (and the cover's analysis), genre,
 * lyrics and rhythm map.
 *
 * Runs while coverwave is open, one song at a time, only on Wi-Fi unless
 * mobile data is allowed, and stops at the first sign of being offline. The
 * library is listed again every 12 hours.
 */
class OfflineSaver(private val model: AppModel) {
    /** What it is doing, for Settings. */
    var activity by mutableStateOf<String?>(null)
        private set

    /** Songs with everything saved, of those known. */
    var ready by mutableIntStateOf(0)
        private set
    var known by mutableIntStateOf(0)
        private set
    var diskBytes by mutableLongStateOf(0L)
        private set

    private val scope = MainScope()
    private var job: Job? = null
    private val relistEveryMs = 12L * 3600 * 1000
    private val lastListKey = "offline.lastList"

    private val settings get() = model.settings

    /** Whether the network may be used for saving now. */
    private val allowed: Boolean
        get() = settings.saveAhead != SaveScope.OFF && model.activeKind != SourceKind.OFFLINE &&
            NetworkMonitor.isOnline && (settings.saveOnMobileData || !NetworkMonitor.isExpensive)

    /** Starts a pass if one is due and allowed (cheap to call often). */
    fun kick(force: Boolean = false) {
        refreshCounts()
        val s = model.spotify
        if (job != null || !allowed || !s.isConfigured || s.status != SourceStatus.Connected || s.followingApp) return
        job = scope.launch {
            try {
                pass(force)
            } finally {
                job = null
                activity = null
                refreshCounts()
            }
        }
    }

    fun stop() {
        job?.cancel()
        job = null
        activity = null
    }

    /** Forgets everything saved for offline. */
    fun deleteAll() {
        stop()
        OfflineStore.clear()
        Platform.prefs.edit().remove(lastListKey).apply()
        refreshCounts()
    }

    fun refreshCounts() {
        val records = OfflineStore.all()
        known = records.size
        ready = records.count(::isComplete)
        scope.launch {
            diskBytes = withContext(Dispatchers.IO) { OfflineStore.diskBytes() }
        }
    }

    private fun isComplete(r: OfflineStore.Record) =
        r.coverChecked && r.genreChecked && r.lyricsChecked && (r.rhythmChecked || !settings.tempoLookup)

    private suspend fun pass(force: Boolean) {
        val last = Platform.prefs.getLong(lastListKey, 0)
        if (force || System.currentTimeMillis() - last > relistEveryMs) {
            if (!listLibrary()) return
            Platform.prefs.edit().putLong(lastListKey, System.currentTimeMillis()).apply()
        }
        val todo = OfflineStore.all().filterNot(::isComplete).sortedBy { it.title }
        for ((i, r) in todo.withIndex()) {
            if (!scope.isActive || !allowed) return
            activity = "Saving ${i + 1} of ${todo.size}: ${r.title}"
            if (!save(r)) return
            if (i % 10 == 0) refreshCounts()
        }
    }

    /** Lists (and so saves) liked songs and, with ALL, playlists. */
    private suspend fun listLibrary(): Boolean {
        val library = model.spotifyLibrary ?: return false
        return try {
            activity = "Listing liked songs"
            library.rows(LibraryPath("liked", "liked songs"))
            if (settings.saveAhead != SaveScope.ALL) return true
            activity = "Listing playlists"
            val playlists = library.rows(LibraryPath("playlists", "playlists"))
            for (p in playlists) {
                if (!allowed) return false
                val id = p.opensID ?: continue
                activity = "Listing ${p.title}"
                runCatching { library.rows(LibraryPath(id, p.opensTitle ?: p.title)) }
            }
            true
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (_: Exception) {
            activity = null
            false
        }
    }

    /** Fills in one song. False when the internet went away. */
    private suspend fun save(r: OfflineStore.Record): Boolean {
        val np = r.nowPlaying
        if (!r.coverChecked) {
            val url = np.artworkURL
            if (url != null && !OfflineStore.hasCover(url)) {
                val response = runCatching { Http.request(url) }.getOrElse { return false }
                if (response.code == 200) {
                    OfflineStore.saveCover(response.body, url)
                    // The cover's analysis is cached too (when the model is ready).
                    if (model.core.modelState == CoverCore.ModelState.Ready) {
                        runCatching { model.core.analyse(response.body, r.genre) }
                    }
                }
            }
            OfflineStore.update(r.id) { it.copy(coverChecked = true) }
        }
        if (!r.genreChecked) {
            val genre = GenreLookup.genre(np)
            OfflineStore.update(r.id) { it.copy(genre = it.genre ?: genre, genreChecked = true) }
        }
        if (!r.lyricsChecked) {
            LyricsLookup.lyrics(np)
            // Saved only when LRCLIB answered.
            if (!LyricsLookup.isSaved(r.id)) return false
            OfflineStore.update(r.id) { it.copy(lyricsChecked = true) }
        }
        if (settings.tempoLookup && !r.rhythmChecked) {
            RhythmLookup.rhythm(np)
            if (!NetworkMonitor.isOnline) return false
            OfflineStore.update(r.id) { it.copy(rhythmChecked = true) }
        }
        return true
    }
}
