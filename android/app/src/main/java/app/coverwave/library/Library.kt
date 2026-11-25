package app.coverwave.library

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * A place in a library: [id] is what the provider resolves ("playlists",
 * "album:123"); [title] is the breadcrumb name.
 */
data class LibraryPath(val id: String, val title: String) {
    companion object {
        val ROOT = LibraryPath("root", "library")
    }
}

enum class QueuePosition(val label: String) {
    NEXT("Play Next"),
    LAST("Add to Queue"),
}

/**
 * One row. Folders open; tracks play (from that track on, through the rest
 * of their album or playlist) and can be queued.
 */
data class LibraryItem(
    val id: String,
    val title: String,
    val subtitle: String? = null,
    /** Right-aligned: a track count or a duration. */
    val detail: String? = null,
    val opens: LibraryPath? = null,
    val play: (suspend () -> Unit)? = null,
    val enqueue: (suspend (QueuePosition) -> Unit)? = null,
)

data class QueueSnapshot(
    val current: Entry?,
    val upcoming: List<Entry>,
    /** Why the list may be incomplete. */
    val note: String? = null,
) {
    data class Entry(val id: String, val title: String, val subtitle: String?)
}

sealed class LibraryException(message: String) : Exception(message) {
    /** The login lacks permission or has expired: log in again. */
    class NeedsLogin : LibraryException("Spotify needs you to log in again for your library.")
    class Message(text: String) : LibraryException(text)
}

interface LibraryProvider {
    val name: String

    /** Which queue positions the source supports. */
    val queuePositions: List<QueuePosition>
    suspend fun list(path: LibraryPath): List<LibraryItem>
    suspend fun search(term: String): List<LibraryItem>
    suspend fun upNext(): QueueSnapshot

    /** Logs in again with the permissions the library needs. */
    fun reauthorize()
}

/**
 * Browsing state for the library panel: a stack of places, a short-lived
 * cache so going back does not refetch, a filter, and search.
 */
class LibraryBrowser(val provider: LibraryProvider) {
    private val scope = MainScope()

    var stack by mutableStateOf(listOf(LibraryPath.ROOT))
        private set
    var items by mutableStateOf<List<LibraryItem>>(emptyList())
        private set
    var loading by mutableStateOf(false)
        private set
    var error by mutableStateOf<LibraryException?>(null)
        private set

    /** Results of a library-wide search (root only). */
    var results by mutableStateOf<List<LibraryItem>>(emptyList())
        private set
    var searching by mutableStateOf(false)
        private set

    /** A short confirmation ("queued: karma police"). */
    var notice by mutableStateOf<String?>(null)
        private set

    var query by mutableStateOf("")
        private set

    private val cache = HashMap<String, Pair<List<LibraryItem>, Long>>()
    private var searchJob: Job? = null
    private var noticeJob: Job? = null
    private var loadJob: Job? = null
    private val ttlMs = 300_000L

    val path: LibraryPath get() = stack.last()
    val atRoot: Boolean get() = stack.size == 1

    /** What to show: the listing, filtered by the query. */
    val rows: List<LibraryItem>
        get() {
            val q = query.trim().lowercase()
            if (q.isEmpty()) return items
            return items.filter { it.title.lowercase().contains(q) || it.subtitle?.lowercase()?.contains(q) == true }
        }

    fun updateQuery(value: String) {
        if (value == query) return
        query = value
        queryChanged()
    }

    fun load(force: Boolean = false) {
        val here = path
        val hit = cache[here.id]
        if (!force && hit != null && System.currentTimeMillis() - hit.second < ttlMs) {
            items = hit.first
            error = null
            return
        }
        loadJob?.cancel()
        loading = true
        error = null
        loadJob = scope.launch {
            try {
                val fetched = provider.list(here)
                cache[here.id] = fetched to System.currentTimeMillis()
                if (path == here) items = fetched
            } catch (e: LibraryException) {
                if (path == here) error = e
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                if (path == here) error = LibraryException.Message(e.message ?: "Something went wrong.")
            } finally {
                if (path == here) loading = false
            }
        }
    }

    fun open(to: LibraryPath) {
        stack = stack + to
        reset()
    }

    fun back() {
        if (stack.size <= 1) return
        stack = stack.dropLast(1)
        reset()
    }

    /** Jumps to a breadcrumb. */
    fun pop(index: Int) {
        if (index >= stack.size - 1) return
        stack = stack.take(index + 1)
        reset()
    }

    private fun reset() {
        query = ""
        results = emptyList()
        items = emptyList()
        load()
    }

    /** Opens a folder, or plays; true when something started playing. */
    suspend fun activate(item: LibraryItem): Boolean {
        item.opens?.let {
            open(it)
            return false
        }
        val play = item.play ?: return false
        return try {
            play()
            true
        } catch (e: Exception) {
            say(e.message ?: "Something went wrong.")
            false
        }
    }

    suspend fun enqueue(item: LibraryItem, position: QueuePosition) {
        val enqueue = item.enqueue ?: return
        try {
            enqueue(position)
            say(if (position == QueuePosition.NEXT) "plays next: ${item.title}" else "queued: ${item.title}")
        } catch (e: Exception) {
            say(e.message ?: "Something went wrong.")
        }
    }

    fun reauthorize() {
        cache.clear()
        provider.reauthorize()
    }

    private fun say(text: String) {
        notice = text
        noticeJob?.cancel()
        noticeJob = scope.launch {
            delay(2500)
            notice = null
        }
    }

    private fun queryChanged() {
        searchJob?.cancel()
        val term = query.trim()
        if (!atRoot || term.length < 2) {
            results = emptyList()
            searching = false
            return
        }
        searching = true
        searchJob = scope.launch {
            delay(300)
            val found = runCatching { provider.search(term) }.getOrDefault(emptyList())
            results = found
            searching = false
        }
    }
}
