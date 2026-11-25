package app.coverwave.ui

import android.text.format.Formatter
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.coverwave.AppModel
import app.coverwave.ArtDetail
import app.coverwave.BuildConfig
import app.coverwave.GestureScheme
import app.coverwave.MotionSetting
import app.coverwave.SaveScope
import app.coverwave.core.CoverCore
import app.coverwave.core.NetworkMonitor
import app.coverwave.core.OfflineStore
import app.coverwave.label
import app.coverwave.sources.SourceKind
import app.coverwave.sources.SourceStatus
import uniffi.cw_ffi.ArtMode
import uniffi.cw_ffi.LyricPaintKind
import uniffi.cw_ffi.LyricScrollKind
import uniffi.cw_ffi.LyricSizeKind
import uniffi.cw_ffi.LyricStyleKind

/** Common choices first; what the cover is made of, one section down. */
@Composable
fun SettingsScreen(model: AppModel, accent: Color, onClose: () -> Unit, onShowGuide: () -> Unit) {
    val s = model.settings
    var confirmDelete by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().swallowTaps().background(Theme.page).safeDrawingPadding().imePadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            TermText("~/settings", size = 16f, weight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            TermText("done", size = 15f, weight = FontWeight.Medium, color = accent, modifier = Modifier.termRow(onClick = onClose).padding(8.dp))
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).padding(bottom = 32.dp)) {
            Section("gestures") {
                Choice("scheme", GestureScheme.entries, s.gestures, { it.label }, accent) { s.gestures = it }
                Box(Modifier.padding(vertical = 8.dp)) { GuideTable(s.gestures) }
                Action("show guide on the cover", accent, onClick = onShowGuide)
            }

            Section(
                "lyrics",
                "Swipe sideways with two fingers to switch between the cover and its lyrics, which take the cover's place with its motion still running. Lyric Size sets how big the sung line is; Detail makes its letters finer without changing their size. Cover colours fill each letter with the cover beneath it, like a window. Lyrics come from LRCLIB (lrclib.net), which is sent the song's title, artist and album.",
            ) {
                Toggle("show lyrics", s.showLyrics, accent) { s.showLyrics = it }
                Choice("style", listOf(LyricStyleKind.BIG_ONLY, LyricStyleKind.BIG_CURRENT, LyricStyleKind.TEXT), s.lyricStyle, { it.label }, accent) { s.lyricStyle = it }
                Choice("scroll", LyricScrollKind.entries, s.lyricScroll, { it.label }, accent) { s.lyricScroll = it }
                Choice("lyric size", LyricSizeKind.entries, s.lyricSize, { it.label }, accent) { s.lyricSize = it }
                Choice("colours", LyricPaintKind.entries, s.lyricPaint, { if (it == LyricPaintKind.COVER) "Cover" else "Accent" }, accent) { s.lyricPaint = it }
            }

            Section(
                "art",
                "Finer detail draws the cover with smaller, more numerous characters. Automatic motion pauses when Remove Animations is on.",
            ) {
                Choice("motion", MotionSetting.entries, s.motion, { it.label }, accent) { s.motion = it }
                Choice("art", ArtMode.entries, s.artMode, { it.label }, accent) { s.artMode = it }
                Choice("detail", ArtDetail.entries, s.artDetail, { it.label }, accent) { s.artDetail = it }
            }

            Section(
                "rhythm",
                "Android keeps other apps' audio private, so coverwave measures each song's rhythm from its 30-second Deezer preview with a beat-tracking model on this phone, then plays that groove in time with the song. Deezer's previews start 48 seconds in, which is how the beat lines up with the song. Finding the preview sends the song's title and artist to Deezer.",
            ) {
                Toggle("measure each song's rhythm", s.tempoLookup, accent) { s.tempoLookup = it }
            }

            val cover = model.cover
            val np = model.nowPlaying
            if (cover != null && np != null) {
                Section("this cover") {
                    Info("song", np.title)
                    Info("genre", AppModel.familyName(cover.genreFamily()).replaceFirstChar { it.uppercase() })
                    Info("motion", cover.layerKinds().joinToString(", ") { it.replace("_", " ") })
                    Info("rhythm", model.tempoText)
                }
            }

            Section(
                "cover analysis",
                "Reads each cover's scene (rain, neon, space…) to choose its motion. Uses Apple's MobileCLIP model (${CoverCore.MODEL_SIZE_TEXT}, research and personal use only), downloaded once from Hugging Face.",
            ) {
                when (val st = model.core.modelState) {
                    CoverCore.ModelState.Ready -> Info("model", "ready")
                    CoverCore.ModelState.Loading -> Info("model", "loading…")
                    is CoverCore.ModelState.Downloading -> Info("model", "downloading ${(st.fraction * 100).toInt()}%")
                    CoverCore.ModelState.Missing -> Action("download model", accent) { model.downloadModel() }
                    is CoverCore.ModelState.Failed -> {
                        Action("try again", accent) { model.downloadModel() }
                        Note(st.message)
                    }
                }
            }

            OfflineSection(model, accent) { confirmDelete = true }

            Section(
                "sources",
                "Tap a source to follow it. Apple Music (and other players) are read through Android's media controls, which needs Notification access for coverwave: the notifications themselves are ignored.",
            ) {
                for (kind in SourceKind.entries) {
                    val source = model.source(kind)
                    val status = when (val st = source.status) {
                        SourceStatus.Connected -> if (model.activeKind == kind) "following" else "connected"
                        SourceStatus.Disconnected -> "not connected"
                        else -> st.message?.lowercase() ?: ""
                    }
                    val enabled = kind == SourceKind.APPLE_MUSIC || model.spotify.isConfigured
                    Row(
                        Modifier.fillMaxWidth().heightIn(min = 44.dp).termRow(enabled) { model.choose(kind); onClose() },
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        TermText(
                            (if (model.activeKind == kind) "> " else "  ") + kind.label.lowercase(),
                            color = if (enabled) Term.ink else Term.faint, modifier = Modifier.weight(1f),
                        )
                        TermText(status, size = 12f, color = Term.dim, maxLines = 2)
                    }
                }
                if (model.spotify.isConfigured && model.spotify.status != SourceStatus.Disconnected) {
                    Action("log out of spotify", accent) { model.spotify.logOut() }
                }
                if (BuildConfig.SPOTIFY_CLIENT_ID.isBlank()) {
                    ClientIDField(model, accent)
                }
            }
        }
    }
    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { TermText("Delete what was saved for offline?", maxLines = 2) },
            text = { TermText("Covers, genres, lyrics, rhythm maps and library listings. Spotify's downloads are not touched.", size = 13f, color = Term.dim, maxLines = 4) },
            confirmButton = {
                TextButton({ model.saver.deleteAll(); confirmDelete = false }) { TermText("delete", color = Color(0xFFE5484D)) }
            },
            dismissButton = { TextButton({ confirmDelete = false }) { TermText("cancel", color = Term.dim) } },
            containerColor = Theme.page,
        )
    }
}

@Composable
private fun OfflineSection(model: AppModel, accent: Color, onDelete: () -> Unit) {
    val s = model.settings
    val saver = model.saver
    val context = LocalContext.current
    @Suppress("UNUSED_VARIABLE") val rev = OfflineStore.revision
    LaunchedEffect(Unit) { saver.refreshCounts() }
    val blocked = when {
        s.saveAhead == SaveScope.OFF -> "Turn on Save Ahead first."
        saver.activity != null -> "Saving now."
        model.isOfflineMode -> "Saving needs the Spotify source: switch from Offline to Spotify in Sources."
        !NetworkMonitor.isOnline -> "Saving needs the internet."
        NetworkMonitor.isExpensive && !s.saveOnMobileData -> "On mobile data: turn on Use Mobile Data, or join Wi-Fi."
        model.spotify.status != SourceStatus.Connected -> "Connect Spotify first."
        else -> null
    }
    Section(
        "offline",
        "Pick Offline on the start screen (or in Sources) to follow the Spotify app on this phone directly while it plays your downloads. coverwave then looks nothing up and uses only what it saved here, so it uses next to no data. Spotify must be open, and it needs the internet to accept the connection (Spotify's rule); after that the downloads play as usual. While online, Save Ahead keeps each song's cover, genre, lyrics and rhythm, and your library's listings. The songs themselves stay in Spotify.",
    ) {
        Choice("save ahead", SaveScope.entries, s.saveAhead, { it.label }, accent) {
            s.saveAhead = it
            if (it == SaveScope.OFF) saver.stop() else saver.kick(force = true)
        }
        Toggle("use mobile data", s.saveOnMobileData, accent) {
            s.saveOnMobileData = it
            saver.kick()
        }
        Info("saved", "${saver.ready} of ${saver.known} songs · ${Formatter.formatShortFileSize(context, saver.diskBytes)}")
        saver.activity?.let { Note(it) }
        Action("save now", accent, enabled = blocked == null) { saver.kick(force = true) }
        blocked?.let { Note(it) }
        Action("delete saved data", Color(0xFFE5484D), onClick = onDelete)
    }
}

@Composable
private fun ClientIDField(model: AppModel, accent: Color) {
    var text by remember { mutableStateOf(model.settings.spotifyClientID) }
    Column(Modifier.padding(top = 12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        TermText("spotify client id", size = 13f, color = Term.dim)
        Row(verticalAlignment = Alignment.CenterVertically) {
            TermText("> ", color = accent)
            Box(Modifier.weight(1f)) {
                if (text.isEmpty()) TermText("paste it here", color = Term.faint)
                BasicTextField(text, { text = it.trim() }, textStyle = Term.style(14f), singleLine = true, cursorBrush = SolidColor(accent), modifier = Modifier.fillMaxWidth())
            }
            Spacer(Modifier.width(8.dp))
            TermText("save", size = 14f, weight = FontWeight.Medium, color = accent, modifier = Modifier.termRow {
                model.settings.spotifyClientID = text
                model.spotify.clientIDChanged()
            }.padding(6.dp))
        }
        Note(
            "From developer.spotify.com > your app. Add the redirect URI coverwave://spotify-callback, and under Android packages: app.coverwave with SHA-1 1F:C1:54:69:E7:2A:EB:2D:2A:42:44:F8:C3:AD:0E:28:49:38:74:32.",
        )
    }
}

@Composable
private fun Section(title: String, footer: String? = null, content: @Composable () -> Unit) {
    Column(Modifier.padding(top = 24.dp)) {
        TermText("# $title", size = 13f, weight = FontWeight.SemiBold, color = Term.dim, modifier = Modifier.padding(bottom = 6.dp))
        content()
        footer?.let { Box(Modifier.padding(top = 8.dp)) { Note(it) } }
    }
}

@Composable
private fun Note(text: String) = TermText(text, size = 12f, color = Term.dim, maxLines = 20)

@Composable
private fun Info(label: String, value: String) {
    Row(Modifier.fillMaxWidth().heightIn(min = 36.dp).padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        TermText(label, color = Term.dim, modifier = Modifier.width(96.dp))
        TermText(value, maxLines = 3)
    }
}

@Composable
private fun Action(title: String, color: Color, enabled: Boolean = true, onClick: () -> Unit) {
    TermText(
        "> $title", size = 15f, weight = FontWeight.Medium, color = if (enabled) color else Term.faint,
        modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp).termRow(enabled, onClick).padding(vertical = 12.dp),
    )
}

@Composable
private fun Toggle(title: String, on: Boolean, accent: Color, set: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 44.dp).termRow { set(!on) }, verticalAlignment = Alignment.CenterVertically) {
        TermText(title, modifier = Modifier.weight(1f))
        TermText(if (on) "[x]" else "[ ]", color = if (on) accent else Term.dim, weight = FontWeight.Medium)
    }
}

/** A picker as a row of words; the chosen one is bracketed in the accent. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun <T> Choice(title: String, options: List<T>, selected: T, label: (T) -> String, accent: Color, set: (T) -> Unit) {
    Column(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
        TermText(title, color = Term.dim, size = 13f)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            for (o in options) {
                val on = o == selected
                TermText(
                    if (on) "[${label(o).lowercase()}]" else " ${label(o).lowercase()} ",
                    color = if (on) accent else Term.ink, weight = if (on) FontWeight.Medium else FontWeight.Normal,
                    modifier = Modifier.heightIn(min = 40.dp).termRow { set(o) }.padding(vertical = 10.dp),
                )
            }
        }
    }
}

