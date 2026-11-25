package app.coverwave.sources

import android.graphics.Bitmap
import app.coverwave.core.NetworkMonitor
import app.coverwave.core.Platform
import app.coverwave.library.LibraryException
import com.spotify.android.appremote.api.ConnectionParams
import com.spotify.android.appremote.api.Connector
import com.spotify.android.appremote.api.SpotifyAppRemote
import com.spotify.protocol.client.CallResult
import com.spotify.protocol.types.Image
import com.spotify.protocol.types.PlayerState
import com.spotify.protocol.types.Track
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import java.io.ByteArrayOutputStream
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/**
 * A direct line to the Spotify app on this phone (Spotify's App Remote SDK),
 * for when the Web API cannot be reached: it talks to the app itself, so it
 * follows and controls songs Spotify plays from its downloads.
 *
 * On Android the Spotify app approves coverwave itself (`showAuthView`), once,
 * which needs this app's package name and signing fingerprint registered in
 * the Spotify developer dashboard. Spotify checks each connection with its
 * servers, so connecting needs the internet; playing downloads afterwards
 * does not.
 */
class SpotifyAppLink(private val clientID: String) {
    sealed class State {
        data object Idle : State()
        data object Connecting : State()
        data object Connected : State()
        data class Failed(val message: String) : State()
    }

    companion object {
        const val NOT_PREPARED =
            "Spotify only accepts a connection while it can reach its servers. Connect to the internet (Spotify can stay in its own offline mode), then tap offline again."
    }

    var state: State = State.Idle
        private set(value) {
            if (field != value) {
                field = value
                onState?.invoke(value)
            }
        }
    var onState: ((State) -> Unit)? = null

    /** Each player state the app reports. */
    var onTrack: ((NowPlaying) -> Unit)? = null

    private var remote: SpotifyAppRemote? = null
    private var currentTrack: Track? = null
    private val scope = MainScope()
    private var attempt = 0

    val isConnected: Boolean get() = remote?.isConnected == true

    fun connect(interactive: Boolean) {
        if (isConnected) {
            state = State.Connected
            return
        }
        if (!SpotifyAppRemote.isSpotifyInstalled(Platform.context)) {
            state = State.Failed("Install Spotify to use it offline")
            return
        }
        state = State.Connecting
        val mine = ++attempt
        val params = ConnectionParams.Builder(clientID)
            .setRedirectUri(SpotifySource.REDIRECT_URI)
            .showAuthView(interactive && NetworkMonitor.isOnline)
            .build()
        SpotifyAppRemote.connect(Platform.context, params, object : Connector.ConnectionListener {
            override fun onConnected(r: SpotifyAppRemote) {
                Platform.onMain {
                    if (mine != attempt) return@onMain SpotifyAppRemote.disconnect(r)
                    remote = r
                    state = State.Connected
                    r.playerApi.subscribeToPlayerState()
                        .setEventCallback { received(it) }
                        .setErrorCallback { e -> Platform.onMain { state = State.Failed(explain(e)) } }
                }
            }

            override fun onFailure(e: Throwable) {
                Platform.onMain { if (mine == attempt) state = State.Failed(explain(e)) }
            }
        })
        // Never just say "connecting".
        scope.launch {
            delay(if (interactive) 90_000 else 12_000)
            if (mine == attempt && state == State.Connecting) {
                state = State.Failed(
                    if (NetworkMonitor.isOnline) "Spotify didn't answer. Open Spotify and play something, then tap offline again."
                    else NOT_PREPARED,
                )
            }
        }
    }

    fun disconnect() {
        attempt++
        remote?.let { SpotifyAppRemote.disconnect(it) }
        remote = null
        currentTrack = null
        state = State.Idle
    }

    private fun received(s: PlayerState) {
        val t = s.track ?: return
        val np = NowPlaying(
            trackID = t.uri,
            title = t.name ?: "",
            artist = t.artist?.name ?: "",
            album = t.album?.name ?: "",
            duration = if (t.duration > 0) t.duration / 1000.0 else null,
            position = s.playbackPosition / 1000.0,
            readAt = System.currentTimeMillis(),
            isPlaying = !s.isPaused,
        )
        Platform.onMain {
            currentTrack = t
            onTrack?.invoke(np)
        }
    }

    // Controls

    fun playPause(isPlaying: Boolean) {
        remote?.playerApi?.let { if (isPlaying) it.pause() else it.resume() }
    }

    fun next() {
        remote?.playerApi?.skipNext()
    }

    fun previous() {
        remote?.playerApi?.skipPrevious()
    }

    fun seek(seconds: Double) {
        remote?.playerApi?.seekTo((seconds * 1000).toLong())
    }

    private suspend fun <T> await(result: CallResult<T>): T = suspendCancellableCoroutine { c ->
        result.setResultCallback { c.resume(it) }
        result.setErrorCallback { c.resumeWithException(LibraryException.Message(explain(it))) }
    }

    private fun api() = remote?.takeIf { it.isConnected }?.playerApi
        ?: throw LibraryException.Message("Spotify isn't connected. Open Spotify, then try again.")

    /** Plays a track, album or playlist URI. */
    suspend fun play(uri: String) {
        await(api().play(uri))
    }

    /** Plays a context from its track at [index] (else the track, or the whole context). */
    suspend fun play(context: String, index: Int?, track: String?) {
        if (index != null) {
            val ok = runCatching {
                await(api().play(context))
                await(api().skipToIndex(context, index))
            }.isSuccess
            if (ok) return
        }
        play(track ?: context)
    }

    suspend fun enqueue(uri: String) {
        await(api().queue(uri))
    }

    /** The playing track's cover, from the Spotify app's own copy. */
    suspend fun image(trackID: String): ByteArray? {
        val t = currentTrack?.takeIf { it.uri == trackID } ?: return null
        val images = remote?.imagesApi ?: return null
        val bitmap: Bitmap = runCatching { await(images.getImage(t.imageUri, Image.Dimension.LARGE)) }
            .getOrNull() ?: return null
        return ByteArrayOutputStream().use {
            bitmap.compress(Bitmap.CompressFormat.JPEG, 92, it)
            it.toByteArray()
        }
    }

    /** Spotify's own reason, with what to do about it. */
    fun explain(e: Throwable?): String {
        val reasons = generateSequence(e) { it.cause }.mapNotNull { it.message ?: it.javaClass.simpleName }
            .distinct().toList()
        val reason = reasons.joinToString(": ").ifEmpty { "no reason given" }
        val lower = reason.lowercase()
        if (lower.contains("auth") || lower.contains("token") || lower.contains("explicit user")) {
            return if (NetworkMonitor.isOnline) "Spotify's player turned down coverwave ($reason). Tap offline to approve it in Spotify."
            else "Spotify's player needs the internet to accept coverwave ($reason). Connect to the internet, then tap offline again."
        }
        if (lower.contains("not installed")) return "Install Spotify to use it offline"
        return "Couldn't reach Spotify's player ($reason). Open Spotify and play a song (offline: a downloaded one) so it stays awake, then tap again."
    }
}
