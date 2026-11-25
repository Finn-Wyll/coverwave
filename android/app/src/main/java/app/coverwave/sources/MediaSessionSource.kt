package app.coverwave.sources

import android.content.ComponentName
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.MediaMetadata
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.net.Uri
import android.os.SystemClock
import android.provider.Settings
import android.service.notification.NotificationListenerService
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.app.NotificationManagerCompat
import app.coverwave.core.Http
import app.coverwave.core.Platform
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream

/**
 * Lets coverwave read other apps' media sessions. Android only hands those
 * to an enabled notification listener; notifications themselves are
 * ignored.
 */
class MediaListener : NotificationListenerService()

/**
 * Follows Apple Music (or, when it is not playing, any other player) through
 * Android's media sessions: title, artist, album, genre, position, the
 * cover, and play/pause/skip/seek. Android has no way to browse Apple
 * Music's library from another app, so the library panel explains that;
 * the queue is the player's own, when it shares one.
 */
class MediaSessionSource : MusicSource {
    companion object {
        const val APPLE_MUSIC = "com.apple.android.music"
    }

    override val kind = SourceKind.APPLE_MUSIC
    override var status by mutableStateOf<SourceStatus>(SourceStatus.Disconnected)
        private set
    override var nowPlaying by mutableStateOf<NowPlaying?>(null)
        private set
    override var onChange: ((NowPlaying?) -> Unit)? = null

    /** The app being followed ("Apple Music", "YouTube Music"...). */
    var playerName by mutableStateOf<String?>(null)
        private set

    private val context get() = Platform.context
    private val manager get() = context.getSystemService(MediaSessionManager::class.java)
    private val listener = ComponentName(Platform.context, MediaListener::class.java)
    private var controller: MediaController? = null
    private var started = false

    private val sessionsChanged = MediaSessionManager.OnActiveSessionsChangedListener { pick(it.orEmpty()) }

    private val callback = object : MediaController.Callback() {
        override fun onMetadataChanged(metadata: MediaMetadata?) = Platform.onMain { refresh() }
        override fun onPlaybackStateChanged(state: PlaybackState?) = Platform.onMain {
            refresh()
            // Another player may have started while this one stopped.
            if (state?.state != PlaybackState.STATE_PLAYING) pick(activeSessions())
        }
        override fun onSessionDestroyed() = Platform.onMain { pick(activeSessions()) }
    }

    private val granted: Boolean
        get() = NotificationManagerCompat.getEnabledListenerPackages(context).contains(context.packageName)

    override fun connect() {
        if (granted) return start()
        status = SourceStatus.NeedsAuthorization
        runCatching {
            context.startActivity(
                Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }

    override fun resume() {
        if (granted) start()
        else if (status != SourceStatus.Disconnected) {
            status = SourceStatus.Unavailable("Allow coverwave under Notification access, then tap apple music")
        }
    }

    private fun activeSessions(): List<MediaController> =
        runCatching { manager.getActiveSessions(listener) }.getOrDefault(emptyList())

    private fun start() {
        if (!started) {
            runCatching { manager.addOnActiveSessionsChangedListener(sessionsChanged, listener) }
                .onFailure {
                    status = SourceStatus.Failed("Android refused the media sessions: ${it.message}")
                    return
                }
            started = true
        }
        status = SourceStatus.Connected
        pick(activeSessions())
    }

    override fun disconnect() {
        if (started) runCatching { manager.removeOnActiveSessionsChangedListener(sessionsChanged) }
        started = false
        controller?.unregisterCallback(callback)
        controller = null
        playerName = null
        status = SourceStatus.Disconnected
        update(null)
    }

    /** Apple Music if it has a session; else whatever is playing; else the first. */
    private fun pick(sessions: List<MediaController>) {
        Platform.onMain {
            val playing = sessions.filter { it.playbackState?.state == PlaybackState.STATE_PLAYING }
            val chosen = sessions.firstOrNull { it.packageName == APPLE_MUSIC && it.playbackState?.state == PlaybackState.STATE_PLAYING }
                ?: playing.firstOrNull()
                ?: sessions.firstOrNull { it.packageName == APPLE_MUSIC }
                ?: sessions.firstOrNull()
            if (chosen?.sessionToken != controller?.sessionToken) {
                controller?.unregisterCallback(callback)
                controller = chosen
                chosen?.registerCallback(callback)
                playerName = chosen?.let { appName(it.packageName) }
            }
            refresh()
        }
    }

    private fun appName(pkg: String): String = runCatching {
        val pm = context.packageManager
        pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString()
    }.getOrDefault(pkg)

    private fun refresh() {
        val c = controller
        val m = c?.metadata
        val title = m?.getString(MediaMetadata.METADATA_KEY_TITLE)
        if (c == null || m == null || title.isNullOrEmpty()) return update(null)
        val state = c.playbackState
        val playing = state?.state == PlaybackState.STATE_PLAYING
        var position = (state?.position ?: 0L) / 1000.0
        if (playing && state != null && state.lastPositionUpdateTime > 0) {
            position += (SystemClock.elapsedRealtime() - state.lastPositionUpdateTime) * state.playbackSpeed / 1000.0
        }
        val artist = m.getString(MediaMetadata.METADATA_KEY_ARTIST)
            ?: m.getString(MediaMetadata.METADATA_KEY_ALBUM_ARTIST) ?: ""
        val album = m.getString(MediaMetadata.METADATA_KEY_ALBUM) ?: ""
        val id = m.getString(MediaMetadata.METADATA_KEY_MEDIA_ID)?.takeIf { it.isNotEmpty() }
            ?: "$title|$artist|$album"
        val duration = m.getLong(MediaMetadata.METADATA_KEY_DURATION).takeIf { it > 0 }?.div(1000.0)
        update(
            NowPlaying(
                trackID = "media:${c.packageName}:$id",
                title = title,
                artist = artist,
                album = album,
                duration = duration,
                position = duration?.let { minOf(position, it) } ?: position,
                readAt = System.currentTimeMillis(),
                isPlaying = playing,
                genre = m.getString(MediaMetadata.METADATA_KEY_GENRE)?.takeIf { it.isNotBlank() },
            ),
        )
    }

    private fun update(np: NowPlaying?) {
        val old = nowPlaying
        if (old != null && np != null && old.matches(np)) return
        if (old == null && np == null) return
        nowPlaying = np
        onChange?.invoke(np)
    }

    /** The player's cover: its bitmap, else its art URI. */
    override suspend fun artwork(track: NowPlaying): ByteArray? {
        val m = controller?.metadata?.takeIf { nowPlaying?.trackID == track.trackID } ?: return null
        val bitmap = m.getBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART) ?: m.getBitmap(MediaMetadata.METADATA_KEY_ART)
        if (bitmap != null) return withContext(Dispatchers.Default) { png(bitmap) }
        val uri = m.getString(MediaMetadata.METADATA_KEY_ALBUM_ART_URI) ?: m.getString(MediaMetadata.METADATA_KEY_ART_URI)
        ?: return null
        return withContext(Dispatchers.IO) {
            if (uri.startsWith("http")) Http.bytes(uri)
            else runCatching {
                context.contentResolver.openInputStream(Uri.parse(uri))?.use { s ->
                    BitmapFactory.decodeStream(s)?.let { png(it) }
                }
            }.getOrNull()
        }
    }

    private fun png(b: Bitmap): ByteArray = ByteArrayOutputStream().use {
        b.compress(Bitmap.CompressFormat.PNG, 100, it)
        it.toByteArray()
    }

    override fun playPause() {
        val c = controller ?: return
        if (c.playbackState?.state == PlaybackState.STATE_PLAYING) c.transportControls.pause()
        else c.transportControls.play()
    }

    override fun next() {
        controller?.transportControls?.skipToNext()
    }

    override fun previous() {
        controller?.transportControls?.skipToPrevious()
    }

    override fun seek(seconds: Double) {
        controller?.transportControls?.seekTo((seconds * 1000).toLong())
    }

    /** The player's own queue, when it shares one: (title, subtitle) pairs. */
    fun queue(): List<Pair<String, String?>>? = controller?.queue?.mapNotNull { item ->
        val d = item.description
        d.title?.toString()?.let { it to d.subtitle?.toString() }
    }
}
