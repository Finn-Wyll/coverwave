package app.coverwave.sources

import android.content.Intent
import android.net.Uri
import android.util.Base64
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import app.coverwave.core.Http
import app.coverwave.core.HttpResponse
import app.coverwave.core.NetworkMonitor
import app.coverwave.core.OfflineStore
import app.coverwave.core.Platform
import app.coverwave.core.Vault
import app.coverwave.core.arr
import app.coverwave.core.isOffline
import app.coverwave.core.obj
import app.coverwave.core.objects
import app.coverwave.core.str
import app.coverwave.library.LibraryException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject
import java.security.MessageDigest
import java.security.SecureRandom

/**
 * Follows and controls Spotify through the Web API, like the terminal and
 * iPhone apps.
 *
 * Login is Spotify's authorization-code flow with PKCE (no client secret)
 * in a Custom Tab, returning to `coverwave://spotify-callback`. The refresh
 * token is kept encrypted with a Keystore key, and access tokens are
 * renewed with it, so one login lasts until you log out.
 *
 * What is playing comes from polling `GET /me/player` while coverwave is
 * open. Controls use the player endpoints (Premium).
 *
 * Offline (Offline mode, or whenever the Web API cannot be reached) it
 * follows the Spotify app on this phone directly instead ([SpotifyAppLink]),
 * which plays its downloads; what the Web API would add (genre, cover)
 * comes from [OfflineStore].
 */
class SpotifySource(private val clientIDProvider: () -> String?) : MusicSource {
    companion object {
        const val REDIRECT_URI = "coverwave://spotify-callback"
        val SCOPES = listOf(
            "user-read-playback-state", "user-modify-playback-state", "user-read-currently-playing",
            "user-library-read", "playlist-read-private", "playlist-read-collaborative",
        )
        private const val REFRESH_KEY = "spotify.refresh-token"
        private const val API = "https://api.spotify.com/v1"
        private const val ACCOUNTS = "https://accounts.spotify.com"
        private const val POLL_MS = 1000L

        /** After the Web API could not be reached, follow the app this long. */
        private const val RETRY_ONLINE_MS = 30_000L
    }

    /** A Spotify Connect device (this phone, a computer, a speaker...). */
    data class Device(val id: String, val name: String, val type: String, val isActive: Boolean) {
        val isPhone: Boolean get() = type.lowercase() == "smartphone"
    }

    override val kind = SourceKind.SPOTIFY
    override var status by mutableStateOf<SourceStatus>(SourceStatus.Disconnected)
        private set
    override var nowPlaying by mutableStateOf<NowPlaying?>(null)
        private set

    /** Where Spotify is playing now (from the last poll). */
    var device by mutableStateOf<Device?>(null)
        private set
    override var onChange: ((NowPlaying?) -> Unit)? = null

    private val scope = MainScope()
    private var access: Pair<String, Long>? = null
    private var refreshing: CompletableDeferred<String>? = null
    private var poller: Job? = null
    private val artistGenres = HashMap<String, String>()
    private var pkce: Pair<String, String>? = null // verifier, state

    /** Offline mode: never use the internet, follow the Spotify app. */
    var forceOffline = false
        set(value) {
            if (field != value) {
                field = value
                reroute(interactive = false)
            }
        }

    /** Following the Spotify app (not the Web API) right now. */
    var followingApp by mutableStateOf(false)
        private set
    private var link: SpotifyAppLink? = null
    private var webDownUntil = 0L

    /** Whether to follow the app rather than the Web API. */
    val shouldFollowApp: Boolean
        get() = forceOffline || !NetworkMonitor.isOnline || webDownUntil > System.currentTimeMillis()

    val clientID: String? get() = clientIDProvider()?.trim()?.takeIf { it.isNotEmpty() }

    /** Whether a client ID is set (built in, or entered in Settings). */
    val isConfigured: Boolean get() = clientID != null

    private val hasLogin: Boolean get() = Vault.read(REFRESH_KEY) != null

    init {
        if (!isConfigured) status = SourceStatus.Unavailable("Add your Spotify client ID in Settings")
        NetworkMonitor.onChange += {
            if (status != SourceStatus.Disconnected) {
                if (NetworkMonitor.isOnline) webDownUntil = 0
                reroute(interactive = false)
            }
        }
    }

    /** Settings changed the client ID. */
    fun clientIDChanged() {
        disconnect()
        link?.disconnect()
        link = null
        status = if (isConfigured) SourceStatus.Disconnected
        else SourceStatus.Unavailable("Add your Spotify client ID in Settings")
    }

    // Offline: the Spotify app

    /** Switches between the Web API and the Spotify app as needed. */
    private fun reroute(interactive: Boolean) {
        if (!isConfigured || (status == SourceStatus.Disconnected && !interactive)) return
        if (shouldFollowApp) {
            if (followingApp && link?.isConnected == true) return
            poller?.cancel()
            poller = null
            followApp(interactive)
        } else if (followingApp) {
            link?.disconnect()
            followingApp = false
            if (hasLogin) startPolling() else status = SourceStatus.NeedsAuthorization
        }
    }

    private fun followApp(interactive: Boolean) {
        val id = clientID ?: return
        followingApp = true
        val l = link ?: makeLink(id)
        status = SourceStatus.Connecting
        l.connect(interactive)
    }

    private fun makeLink(clientID: String): SpotifyAppLink {
        val l = SpotifyAppLink(clientID)
        l.onState = { state ->
            if (followingApp) {
                status = when (state) {
                    SpotifyAppLink.State.Connected -> SourceStatus.Connected
                    SpotifyAppLink.State.Connecting -> SourceStatus.Connecting
                    SpotifyAppLink.State.Idle -> SourceStatus.NeedsAuthorization
                    is SpotifyAppLink.State.Failed -> SourceStatus.Failed(state.message)
                }
            }
        }
        l.onTrack = { np -> if (followingApp) update(OfflineStore.enrich(np)) }
        link = l
        return l
    }

    /** The Web API failed to reach Spotify: follow the app for a while. */
    private fun webUnreachable() {
        webDownUntil = System.currentTimeMillis() + RETRY_ONLINE_MS
        reroute(interactive = false)
    }

    // Connecting

    override fun connect() {
        if (!isConfigured) return
        when {
            shouldFollowApp -> followApp(interactive = true)
            hasLogin -> startPolling()
            else -> logIn()
        }
    }

    override fun resume() {
        if (!isConfigured) return
        when {
            shouldFollowApp -> followApp(interactive = false)
            hasLogin -> startPolling()
        }
    }

    override fun disconnect() {
        poller?.cancel()
        poller = null
        link?.disconnect()
        followingApp = false
        status = SourceStatus.Disconnected
        update(null)
    }

    /** Forgets the saved login. */
    fun logOut() {
        disconnect()
        access = null
        Vault.delete(REFRESH_KEY)
    }

    /** Logs in again (from the library's "log in again" row). */
    fun reauthorize() {
        logOut()
        logIn()
    }

    private fun startPolling() {
        if (poller != null) return
        status = SourceStatus.Connecting
        poller = scope.launch {
            while (isActive) {
                poll()
                delay(POLL_MS)
            }
        }
    }

    // Login (PKCE)

    private fun random(n: Int): String {
        val chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~"
        val r = SecureRandom()
        return (1..n).map { chars[r.nextInt(chars.length)] }.joinToString("")
    }

    private fun logIn() {
        val id = clientID ?: return
        status = SourceStatus.Connecting
        val verifier = random(64)
        val challenge = Base64.encodeToString(
            MessageDigest.getInstance("SHA-256").digest(verifier.toByteArray()),
            Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING,
        )
        val state = random(16)
        pkce = verifier to state
        // Kept in case Android stops coverwave while the browser is open.
        Platform.prefs.edit().putString("spotify.pkce", "$verifier $state").apply()
        val url = Http.withQuery(
            "$ACCOUNTS/authorize",
            listOf(
                "client_id" to id, "response_type" to "code", "redirect_uri" to REDIRECT_URI,
                "code_challenge_method" to "S256", "code_challenge" to challenge, "state" to state,
                "scope" to SCOPES.joinToString(" "),
            ),
        )
        runCatching {
            CustomTabsIntent.Builder().build().apply {
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }.launchUrl(Platform.context, Uri.parse(url))
        }.onFailure {
            runCatching {
                Platform.context.startActivity(
                    Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                )
            }.onFailure { status = SourceStatus.Failed("Could not open the Spotify login") }
        }
    }

    /** The login's return (`coverwave://spotify-callback?...`). False for other URIs. */
    fun handle(uri: Uri): Boolean {
        if (uri.scheme != "coverwave" || uri.host != "spotify-callback") return false
        val saved = pkce ?: Platform.prefs.getString("spotify.pkce", null)?.split(" ")
            ?.takeIf { it.size == 2 }?.let { it[0] to it[1] }
        val code = uri.getQueryParameter("code")
        if (saved == null || code == null || uri.getQueryParameter("state") != saved.second) {
            status = if (uri.getQueryParameter("error") == "access_denied") SourceStatus.NeedsAuthorization
            else SourceStatus.Failed("Spotify login failed")
            return true
        }
        pkce = null
        Platform.prefs.edit().remove("spotify.pkce").apply()
        scope.launch {
            try {
                tokenRequest(
                    mapOf(
                        "grant_type" to "authorization_code", "code" to code,
                        "redirect_uri" to REDIRECT_URI, "code_verifier" to saved.first,
                    ),
                )
                startPolling()
            } catch (e: Exception) {
                status = SourceStatus.Failed(e.message ?: "Spotify login failed")
            }
        }
        return true
    }

    /** Posts to the token endpoint and keeps what comes back. */
    private suspend fun tokenRequest(form: Map<String, String>): String {
        val id = clientID ?: throw LibraryException.NeedsLogin()
        val body = (form + ("client_id" to id)).entries.joinToString("&") { "${Http.enc(it.key)}=${Http.enc(it.value)}" }
        val r = Http.request(
            "$ACCOUNTS/api/token", "POST",
            mapOf("Content-Type" to "application/x-www-form-urlencoded"), body.toByteArray(),
        )
        val json = r.json()
        val token = json.str("access_token")
        if (r.code != 200 || token == null) {
            // 400 invalid_grant: the refresh token was revoked or expired.
            if (r.code == 400) {
                Vault.delete(REFRESH_KEY)
                throw LibraryException.NeedsLogin()
            }
            throw LibraryException.Message("Spotify login: HTTP ${r.code}")
        }
        val expires = System.currentTimeMillis() + (json.optDouble("expires_in", 3600.0) - 60) .toLong() * 1000
        access = token to expires
        json.str("refresh_token")?.let { Vault.save(it, REFRESH_KEY) }
        return token
    }

    /** A valid access token, renewed with the refresh token when needed. */
    suspend fun accessToken(): String {
        access?.let { (t, exp) -> if (exp > System.currentTimeMillis()) return t }
        refreshing?.let { return it.await() }
        val refresh = Vault.read(REFRESH_KEY) ?: throw LibraryException.NeedsLogin()
        val d = CompletableDeferred<String>()
        refreshing = d
        try {
            val t = tokenRequest(mapOf("grant_type" to "refresh_token", "refresh_token" to refresh))
            d.complete(t)
            return t
        } catch (e: Exception) {
            d.completeExceptionally(e)
            throw e
        } finally {
            refreshing = null
        }
    }

    // Web API

    /** One Web API call; throws `NeedsLogin` when the login is gone. */
    suspend fun call(method: String, path: String, body: JSONObject? = null): HttpResponse {
        var token = accessToken()
        repeat(2) { attempt ->
            val headers = mutableMapOf("Authorization" to "Bearer $token")
            if (body != null) headers["Content-Type"] = "application/json"
            val r = Http.request(API + path, method, headers, body?.toString()?.toByteArray())
            if (r.code == 401 && attempt == 0) {
                // Expired early: renew once and retry.
                access = null
                token = accessToken()
            } else {
                return r
            }
        }
        throw LibraryException.NeedsLogin()
    }

    // Following

    private suspend fun poll() {
        if (followingApp) return
        try {
            val r = call("GET", "/me/player?additional_types=episode")
            if (status != SourceStatus.Connected) status = SourceStatus.Connected
            when (r.code) {
                200 -> apply(r.json())
                204 -> { // nothing playing anywhere
                    device = null
                    update(null)
                }
                429 -> delay(5000)
            }
        } catch (e: LibraryException.NeedsLogin) {
            poller?.cancel()
            poller = null
            status = SourceStatus.NeedsAuthorization
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            if (e.isOffline) webUnreachable()
            else if (status != SourceStatus.Connected) status = SourceStatus.Failed(e.message ?: "Spotify failed")
        }
    }

    private fun device(d: JSONObject?): Device? {
        val id = d?.str("id") ?: return null
        return Device(id, d.str("name") ?: "Unknown device", d.str("type") ?: "", d.optBoolean("is_active"))
    }

    private suspend fun apply(state: JSONObject) {
        val playingOn = device(state.obj("device"))
        if (playingOn != device) device = playingOn
        val item = state.obj("item")
        val uri = item?.str("uri") ?: return update(null)
        val album = item.obj("album") ?: JSONObject()
        val artists = item.arr("artists")?.objects().orEmpty()
        val names = artists.mapNotNull { it.str("name") }.joinToString(", ")
        val cover = album.arr("images")?.objects()?.maxByOrNull { it.optInt("width") }
        val artistID = artists.firstOrNull()?.str("id")
        var np = NowPlaying(
            trackID = uri,
            title = item.str("name") ?: "Unknown title",
            artist = names.ifEmpty { item.obj("show")?.str("name") ?: "" },
            album = album.str("name") ?: "",
            duration = if (item.has("duration_ms")) item.optDouble("duration_ms") / 1000 else null,
            position = state.optDouble("progress_ms", 0.0) / 1000,
            readAt = System.currentTimeMillis(),
            isPlaying = state.optBoolean("is_playing"),
            isrc = item.obj("external_ids")?.str("isrc"),
            artworkURL = cover?.str("url"),
        )
        if (artistID != null) {
            val known = artistGenres[artistID]
            if (known != null) {
                np = np.copy(genre = known.ifEmpty { null })
            } else {
                // Once per artist; empty for most artists in 2026, in which
                // case AppModel asks Deezer.
                artistGenres[artistID] = ""
                runCatching { call("GET", "/artists/$artistID") }.getOrNull()?.takeIf { it.code == 200 }?.let {
                    val genres = it.json().arr("genres")?.let { a -> (0 until a.length()).map { i -> a.getString(i) } }.orEmpty()
                    artistGenres[artistID] = genres.joinToString(", ")
                    np = np.copy(genre = genres.takeIf { g -> g.isNotEmpty() }?.joinToString(", "))
                }
            }
        }
        OfflineStore.remember(np)
        update(np)
    }

    private fun update(np: NowPlaying?) {
        val old = nowPlaying
        if (old != null && np != null && old.matches(np)) return
        if (old == null && np == null) return
        nowPlaying = np
        onChange?.invoke(np)
    }

    /**
     * The saved cover first (the same bytes online and off, so the same
     * motion), then its URL, then the Spotify app's copy. What is fetched is
     * saved for offline.
     */
    override suspend fun artwork(track: NowPlaying): ByteArray? {
        OfflineStore.cover(track)?.let { return it }
        val url = track.artworkURL
        if (!followingApp && url != null) {
            Http.bytes(url)?.let {
                OfflineStore.saveCover(it, url)
                return it
            }
        }
        link?.image(track.trackID)?.let {
            OfflineStore.saveCover(it, track.trackID)
            return it
        }
        return null
    }

    // Controls

    override fun playPause() {
        val playing = nowPlaying?.isPlaying == true
        nowPlaying?.let {
            // Show it at once; the next poll confirms.
            update(it.copy(position = it.positionAt(), readAt = System.currentTimeMillis(), isPlaying = !playing))
        }
        if (followingApp) return link?.playPause(playing) ?: Unit
        control("PUT", if (playing) "/me/player/pause" else "/me/player/play")
    }

    override fun next() {
        if (followingApp) link?.next() else control("POST", "/me/player/next")
    }

    override fun previous() {
        if (followingApp) link?.previous() else control("POST", "/me/player/previous")
    }

    override fun seek(seconds: Double) {
        if (followingApp) link?.seek(seconds)
        else control("PUT", "/me/player/seek?position_ms=${(seconds * 1000).toLong()}")
    }

    private fun control(method: String, path: String) {
        scope.launch {
            runCatching { send(method, path, null) }
            poll()
        }
    }

    /** Plays a context (album or playlist), from one of its tracks. */
    suspend fun play(context: String, offset: String? = null, index: Int? = null) {
        if (followingApp) {
            val l = link ?: throw LibraryException.Message("Spotify isn't connected.")
            return l.play(context, index, offset)
        }
        val body = JSONObject().put("context_uri", context)
        if (offset != null) body.put("offset", JSONObject().put("uri", offset))
        playBody(body)
    }

    /** Plays tracks in order. Offline the first plays and up to 20 more are queued. */
    suspend fun play(uris: List<String>) {
        val first = uris.firstOrNull() ?: return
        if (followingApp) {
            val l = link ?: throw LibraryException.Message("Spotify isn't connected.")
            l.play(first)
            uris.drop(1).take(20).forEach { runCatching { l.enqueue(it) } }
            return
        }
        playBody(JSONObject().put("uris", JSONArray(uris.take(100))))
    }

    private suspend fun playBody(body: JSONObject) {
        val r = send("PUT", "/me/player/play", body)
        when (r.code) {
            in 200..299 -> poll()
            403 -> throw LibraryException.Message("Spotify Premium is needed to start playback from here.")
            else -> throw LibraryException.Message("Spotify said ${r.code}.")
        }
    }

    /** Adds a track to the end of Spotify's queue. */
    suspend fun enqueue(uri: String) {
        if (followingApp) {
            val l = link ?: throw LibraryException.Message("Spotify isn't connected.")
            return l.enqueue(uri)
        }
        val r = send("POST", "/me/player/queue?uri=${Http.enc(uri)}", null)
        if (r.code !in 200..299) throw LibraryException.Message("Spotify said ${r.code}.")
    }

    /**
     * A player call; with no active device (404), wakes Spotify on this
     * phone, moves playback to it and tries once more.
     */
    private suspend fun send(method: String, path: String, body: JSONObject?): HttpResponse {
        val first = call(method, path, body)
        if (first.code != 404) return first
        val dev = wakeDevice() ?: throw LibraryException.Message("Open Spotify on this phone, then try again.")
        val sep = if (path.contains("?")) "&" else "?"
        return call(method, "$path${sep}device_id=$dev", body)
    }

    private fun openSpotify() {
        runCatching {
            val launch = Platform.context.packageManager.getLaunchIntentForPackage("com.spotify.music")
                ?: Intent(Intent.ACTION_VIEW, Uri.parse("spotify:"))
            Platform.context.startActivity(launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        }
    }

    /** A device to play on: an active one, else this phone's Spotify (woken). */
    private suspend fun wakeDevice(): String? {
        pickDevice()?.let { return it }
        openSpotify()
        repeat(10) {
            delay(1000)
            pickDevice()?.let { return it }
        }
        return null
    }

    // Devices

    /** Every device Spotify can play on now. */
    suspend fun devices(): List<Device> {
        if (followingApp) throw LibraryException.Message("Devices need the internet. Offline, Spotify plays on this phone.")
        val r = call("GET", "/me/player/devices")
        if (r.code != 200) throw LibraryException.Message("Spotify said ${r.code}.")
        return r.json().arr("devices")?.objects()?.mapNotNull { device(it) }.orEmpty()
    }

    /** Moves playback to [device], keeping it playing (or paused) as it was. */
    suspend fun transfer(device: Device) {
        val playing = nowPlaying?.isPlaying ?: false
        val r = call("PUT", "/me/player", JSONObject().put("device_ids", JSONArray(listOf(device.id))).put("play", playing))
        if (r.code !in 200..299) {
            throw LibraryException.Message(if (r.code == 404) "That device went away." else "Spotify said ${r.code}.")
        }
        poll()
    }

    /** Moves playback to this phone, waking its Spotify app if it is not listed. */
    suspend fun transferToThisPhone() {
        devices().firstOrNull { it.isPhone }?.let { return transfer(it) }
        openSpotify()
        repeat(10) {
            delay(1000)
            runCatching { devices() }.getOrNull()?.firstOrNull { it.isPhone }?.let { return transfer(it) }
        }
        throw LibraryException.Message("Spotify on this phone did not wake up. Open it, then try again.")
    }

    private suspend fun pickDevice(): String? {
        val r = runCatching { call("GET", "/me/player/devices") }.getOrNull()?.takeIf { it.code == 200 } ?: return null
        val list = r.json().arr("devices")?.objects().orEmpty()
        val active = list.firstOrNull { it.optBoolean("is_active") }
        val phone = list.firstOrNull { it.str("type")?.lowercase() == "smartphone" }
        return (active ?: phone ?: list.firstOrNull())?.str("id")
    }
}
