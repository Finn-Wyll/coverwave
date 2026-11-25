package app.coverwave

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import app.coverwave.core.CoverCore
import app.coverwave.core.GenreLookup
import app.coverwave.core.LyricsLookup
import app.coverwave.core.LyricsStatus
import app.coverwave.core.NetworkMonitor
import app.coverwave.core.OfflineSaver
import app.coverwave.core.OfflineStore
import app.coverwave.core.RhythmLookup
import app.coverwave.library.LibraryBrowser
import app.coverwave.library.MediaLibrary
import app.coverwave.library.SpotifyLibrary
import app.coverwave.sources.MediaSessionSource
import app.coverwave.sources.MusicSource
import app.coverwave.sources.NowPlaying
import app.coverwave.sources.SourceKind
import app.coverwave.sources.SourceStatus
import app.coverwave.sources.SpotifySource
import app.coverwave.ui.Theme
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.launch
import uniffi.cw_ffi.AnalysedCover
import uniffi.cw_ffi.AudioLevels
import uniffi.cw_ffi.LyricSheet
import uniffi.cw_ffi.Rhythm
import uniffi.cw_ffi.beatLevels
import uniffi.cw_ffi.genreFamily

/**
 * Ties it together, like the iPhone's `AppModel`: follows the active
 * source, analyses each new cover in the Rust core, looks up the genre,
 * lyrics and rhythm, and supplies the audio levels that drive the motion.
 */
class AppModel {
    val settings = AppSettings()
    val core = CoverCore()
    val spotify = SpotifySource { BuildConfig.SPOTIFY_CLIENT_ID.ifBlank { settings.spotifyClientID } }
    val appleMusic = MediaSessionSource()
    val saver by lazy { OfflineSaver(this) }

    private val scope = MainScope()

    var activeKind by mutableStateOf<SourceKind?>(null)
        private set

    /** The analysed cover of the current track (null: idle scene). */
    var cover by mutableStateOf<AnalysedCover?>(null)
        private set

    /** Identity of the shown cover (and genre), for the cross-fade. */
    var coverHash by mutableStateOf<String?>(null)
        private set

    /** The one accent colour, from the cover's palette. */
    var accent by mutableStateOf(Theme.defaultAccent)
        private set

    /** Where the motion's rhythm comes from, for Settings. */
    var tempoText by mutableStateOf("—")
        private set

    private var coverJob: Job? = null
    private var coverFor: Triple<String, String?, String?>? = null
    private val rhythms = HashMap<String, Rhythm?>()
    private val pending = HashSet<String>()
    private val lookedUpGenres = HashMap<String, String?>()
    private val browsers = HashMap<SourceKind, LibraryBrowser>()

    val active: MusicSource? get() = activeKind?.let(::source)

    /** Offline mode: lookups use only what was saved. */
    val isOfflineMode: Boolean get() = activeKind == SourceKind.OFFLINE

    /** Whether lookups (genre, lyrics, rhythm) may use the internet. */
    private val networkAllowed: Boolean get() = !isOfflineMode && NetworkMonitor.isOnline

    /** Spotify's library (also what the saver lists). */
    val spotifyLibrary: SpotifyLibrary?
        get() {
            if (!spotify.isConfigured) return null
            return (browsers.getOrPut(SourceKind.SPOTIFY) { LibraryBrowser(SpotifyLibrary(spotify)) }.provider as? SpotifyLibrary)
        }

    /** What is playing, with a looked-up genre if its source gave none. */
    val nowPlaying: NowPlaying? get() = withGenre(active?.nowPlaying)

    /** Library browsing for the active source (kept per source). */
    val library: LibraryBrowser?
        get() {
            var kind = activeKind ?: return null
            // Offline browses the same (saved) Spotify library.
            if (kind == SourceKind.OFFLINE) kind = SourceKind.SPOTIFY
            return browsers.getOrPut(kind) {
                LibraryBrowser(if (kind == SourceKind.APPLE_MUSIC) MediaLibrary(appleMusic) else SpotifyLibrary(spotify))
            }
        }

    /** A source is connected (the player shows instead of the source chooser). */
    val isFollowing: Boolean get() = active?.status == SourceStatus.Connected

    init {
        spotify.onChange = { np ->
            trackChanged(np, if (activeKind == SourceKind.OFFLINE) SourceKind.OFFLINE else SourceKind.SPOTIFY)
            if (spotify.status == SourceStatus.Connected) saver.kick()
        }
        appleMusic.onChange = { np -> trackChanged(np, SourceKind.APPLE_MUSIC) }
        NetworkMonitor.onChange += { saver.kick() }
    }

    fun start() {
        settings.lastSource?.let { last ->
            activeKind = last
            spotify.forceOffline = last == SourceKind.OFFLINE
            source(last).resume()
        }
        scope.launch {
            core.loadIfPresent()
            if (core.modelState == CoverCore.ModelState.Missing && !isOfflineMode) core.download()
            modelBecameReady()
            saver.kick()
        }
    }

    /** Downloads the cover-analysis model (from Settings, after a failure). */
    fun downloadModel() {
        scope.launch {
            core.download()
            modelBecameReady()
        }
    }

    fun source(kind: SourceKind): MusicSource = when (kind) {
        SourceKind.SPOTIFY, SourceKind.OFFLINE -> spotify
        SourceKind.APPLE_MUSIC -> appleMusic
    }

    /** Makes [kind] the active source (from a tap: it may ask for permission). */
    fun choose(kind: SourceKind) {
        activeKind?.let { current -> if (current != kind && source(current) !== source(kind)) source(current).disconnect() }
        activeKind = kind
        settings.lastSource = kind
        spotify.forceOffline = kind == SourceKind.OFFLINE
        if (kind == SourceKind.OFFLINE) saver.stop()
        source(kind).connect()
        trackChanged(source(kind).nowPlaying, kind)
    }

    fun sceneBecameActive() {
        active?.resume()
        saver.kick()
    }

    // Covers

    private fun withGenre(np: NowPlaying?): NowPlaying? {
        if (np == null || np.genre != null) return np
        val g = lookedUpGenres[np.trackID] ?: return np
        return np.copy(genre = g)
    }

    private fun lookUpGenre(np: NowPlaying, kind: SourceKind) {
        if (np.genre != null || lookedUpGenres.containsKey(np.trackID)) return
        lookedUpGenres[np.trackID] = null
        val saved = OfflineStore.record(np.trackID)
        if (saved != null && (saved.genreChecked || !networkAllowed)) {
            lookedUpGenres[np.trackID] = saved.genre
            return
        }
        if (!networkAllowed) return
        scope.launch {
            val genre = GenreLookup.genre(np)
            lookedUpGenres[np.trackID] = genre
            OfflineStore.update(np.trackID) { it.copy(genre = it.genre ?: genre, genreChecked = true) }
            // Rebuild the cover's recipe for its genre.
            if (genre != null && activeKind == kind && source(kind).nowPlaying?.trackID == np.trackID) {
                trackChanged(source(kind).nowPlaying, kind)
            }
        }
    }

    // Lyrics

    private var lyrics by mutableStateOf<Triple<String, LyricSheet?, LyricsStatus>?>(null)

    /** Lyrics for the song playing now, if found. */
    val lyricSheet: LyricSheet? get() = lyrics?.takeIf { it.first == nowPlaying?.trackID }?.second
    val lyricsStatus: LyricsStatus? get() = lyrics?.takeIf { it.first == nowPlaying?.trackID }?.third

    private fun lookUpLyrics(np: NowPlaying) {
        if (lyrics?.first == np.trackID) return
        lyrics = Triple(np.trackID, null, LyricsStatus.SEARCHING)
        val network = networkAllowed
        scope.launch {
            val (sheet, status) = LyricsLookup.lyrics(np, network)
            if (lyrics?.first == np.trackID) lyrics = Triple(np.trackID, sheet, status)
        }
    }

    private fun trackChanged(raw: NowPlaying?, kind: SourceKind) {
        if (kind != activeKind) return
        if (raw != null) {
            lookUpGenre(raw, kind)
            lookUpLyrics(raw)
        }
        val np = withGenre(raw)
        if (np == null) {
            coverJob?.cancel()
            coverFor = null
            showCover(null)
            return
        }
        lookUpRhythm(np)
        // A new track, or its genre or cover URL arrived. A cover whose
        // embedding is cached rebuilds instantly.
        val key = Triple(np.trackID, np.genre, np.artworkURL)
        if (key == coverFor) return
        coverFor = key
        coverJob?.cancel()
        coverJob = scope.launch {
            val src = active ?: return@launch
            val image = src.artwork(np)
            if (image == null) {
                showCover(null)
                return@launch
            }
            val analysed = runCatching { core.analyse(image, np.genre) }.getOrNull()
            if (nowPlaying?.trackID == np.trackID) showCover(analysed)
        }
    }

    private fun showCover(c: AnalysedCover?) {
        cover = c
        coverHash = c?.let { "${it.hash()}-${it.genreFamily()}" }
        accent = c?.accent()?.let { Color(it.r.toInt(), it.g.toInt(), it.b.toInt()) } ?: Theme.defaultAccent
        refreshTempoText()
    }

    /** Re-analyses the current cover once the model has loaded. */
    private fun modelBecameReady() {
        val kind = activeKind ?: return
        val np = nowPlaying ?: return
        if (core.modelState != CoverCore.ModelState.Ready || cover?.isComplete() != false) return
        coverFor = null
        trackChanged(np, kind)
    }

    // Motion

    private fun lookUpRhythm(np: NowPlaying) {
        if (!settings.tempoLookup || rhythms.containsKey(np.trackID)) return refreshTempoText()
        rhythms[np.trackID] = null
        pending += np.trackID
        refreshTempoText()
        val network = networkAllowed
        scope.launch {
            val r = runCatching { RhythmLookup.rhythm(np, network) }.getOrNull()
            rhythms[np.trackID] = r
            pending -= np.trackID
            refreshTempoText()
        }
    }

    private fun rhythm(trackID: String): Rhythm? = if (settings.tempoLookup) rhythms[trackID] else null

    private fun refreshTempoText() {
        val np = nowPlaying ?: run {
            tempoText = "—"
            return
        }
        val r = rhythm(np.trackID)
        tempoText = when {
            r != null -> "${Math.round(r.bpm())} BPM in ${r.beatsPerBar()}/4, measured from the song" +
                if (r.isAligned()) "" else " (bar start estimated)"
            settings.tempoLookup && np.trackID in pending -> "Measuring the song's rhythm…"
            else -> {
                val family = np.genre?.let { genreFamily(it) } ?: "unknown"
                val typical = cover?.typicalBpm() ?: 110f
                "${Math.round(typical)} BPM, typical for ${familyName(family)}"
            }
        }
    }

    companion object {
        fun familyName(family: String) = when (family) {
            "unknown" -> "an unknown genre"
            "hip_hop" -> "hip-hop"
            "rnb_soul" -> "R&B and soul"
            else -> family.replace("_", " ")
        }

        val SILENCE = AudioLevels(List(6) { 0f }, 0f, 0f, 0uL)
    }

    /**
     * What drives the motion at [nowMs]: the song's rhythm map against the
     * playback position; else a beat clock at the genre's typical tempo.
     */
    fun levels(nowMs: Long): AudioLevels {
        val np = nowPlaying ?: return SILENCE
        val position = np.positionAt(nowMs)
        val r = rhythm(np.trackID)
            ?: return beatLevels(0f, np.genre, position, np.isPlaying)
        return r.levels(position, np.isPlaying, r.defaultDownbeat())
    }
}
