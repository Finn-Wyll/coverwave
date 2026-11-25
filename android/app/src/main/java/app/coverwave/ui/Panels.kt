package app.coverwave.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import app.coverwave.AppModel
import app.coverwave.library.LibraryBrowser
import app.coverwave.library.LibraryException
import app.coverwave.library.LibraryItem
import app.coverwave.library.QueueSnapshot
import app.coverwave.sources.SourceKind
import app.coverwave.sources.SpotifySource
import kotlinx.coroutines.launch
import kotlin.math.abs

/**
 * Drags on a panel's header move the panel: reported while dragging (no
 * velocity) and on release (with velocity, px/s). Vertical only, after a
 * little hysteresis, so a sideways brush does nothing.
 */
private fun Modifier.headerDrag(onDrag: (Float, Float?) -> Unit): Modifier = pointerInput(Unit) {
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false)
        val tracker = VelocityTracker().apply { addPosition(down.uptimeMillis, down.position) }
        var dragging = false
        while (true) {
            val e = awaitPointerEvent()
            val c = e.changes.firstOrNull { it.id == down.id } ?: return@awaitEachGesture
            tracker.addPosition(c.uptimeMillis, c.position)
            val d = c.position - down.position
            if (!dragging && abs(d.y) > viewConfiguration.touchSlop && abs(d.y) > abs(d.x)) dragging = true
            if (!c.pressed) {
                if (dragging) onDrag(d.y, tracker.calculateVelocity().y)
                return@awaitEachGesture
            }
            if (dragging) {
                c.consume()
                onDrag(d.y, null)
            }
        }
    }
}

@Composable
private fun Grabber() {
    Box(Modifier.width(36.dp).height(4.dp).clip(RoundedCornerShape(2.dp)).background(Term.faint))
}

@Composable
private fun TextButtonRow(title: String, accent: Color, onClick: () -> Unit) {
    TermText(
        title, size = 14f, weight = FontWeight.Medium, color = accent,
        modifier = Modifier.heightIn(min = 36.dp).termRow(onClick = onClick).padding(vertical = 8.dp),
    )
}

/**
 * The library, as a terminal listing: a path, a `/` search line, `..` to go
 * up, folders ending in `/`. It rises from the bottom (where the swipe up
 * came from) and goes back down the same way.
 */
@Composable
fun LibraryPanel(browser: LibraryBrowser, accent: Color, onHeaderDrag: (Float, Float?) -> Unit, close: () -> Unit) {
    val scope = rememberCoroutineScope()
    val focus = LocalFocusManager.current
    LaunchedEffect(browser.path) { browser.load() }
    Column(Modifier.fillMaxSize().swallowTaps().background(Theme.page.copy(alpha = 0.97f)).navigationBarsPadding()) {
        Column(
            Modifier.fillMaxWidth().headerDrag(onHeaderDrag).padding(start = 20.dp, end = 20.dp, top = 10.dp, bottom = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { Grabber() }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Row(Modifier.weight(1f).horizontalScroll(rememberScrollState(), reverseScrolling = true)) {
                    TermText(
                        "~/${browser.provider.name}", size = 14f, weight = FontWeight.Medium, color = Term.dim,
                        modifier = Modifier.termRow { browser.pop(0) },
                    )
                    browser.stack.drop(1).forEachIndexed { i, step ->
                        val index = i + 1
                        TermText(
                            "/${step.title.lowercase()}", size = 14f, weight = FontWeight.Medium,
                            color = if (index == browser.stack.size - 1) Term.ink else Term.dim,
                            modifier = Modifier.termRow { browser.pop(index) },
                        )
                    }
                }
                Spacer(Modifier.width(12.dp))
                TermText("esc", size = 13f, color = Term.dim, modifier = Modifier.termRow(onClick = close).semantics { contentDescription = "Close library" })
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                TermText("/", color = accent)
                Spacer(Modifier.width(8.dp))
                Box(Modifier.weight(1f)) {
                    if (browser.query.isEmpty()) {
                        TermText(if (browser.atRoot) "search ${browser.provider.name}" else "filter", color = Term.faint)
                    }
                    BasicTextField(
                        browser.query, browser::updateQuery,
                        textStyle = Term.style(15f), singleLine = true,
                        cursorBrush = SolidColor(accent),
                        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, imeAction = ImeAction.Search),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(Term.faint))
        LazyColumn(Modifier.weight(1f).padding(vertical = 6.dp)) {
            if (!browser.atRoot) item { Row("..", null, null, false) { browser.back() } }
            val error = browser.error
            when {
                error != null -> item {
                    Column(Modifier.padding(horizontal = 20.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        TermText("! ${error.message ?: "Something went wrong."}", size = 14f, maxLines = 4)
                        if (error is LibraryException.NeedsLogin) {
                            TextButtonRow("> log in to ${browser.provider.name} again", accent) { browser.reauthorize() }
                        }
                        TextButtonRow("> try again", accent) { browser.load(force = true) }
                    }
                }
                browser.loading && browser.items.isEmpty() -> items(6) { i ->
                    // A skeleton in glyphs: rows shaped like the listing.
                    TermText(".".repeat(14 + (i * 7) % 12), color = Term.faint, modifier = Modifier.padding(horizontal = 20.dp).height(48.dp).padding(top = 14.dp))
                }
                else -> {
                    val rows = browser.rows
                    items(rows, key = { "r" + it.id }) { ItemRow(browser, it, accent) { scope.launch { if (browser.activate(it)) { focus.clearFocus(); close() } } } }
                    if (browser.atRoot && browser.query.isNotEmpty()) {
                        item {
                            TermText(
                                if (browser.searching) "searching..." else if (browser.results.isEmpty()) "no matches" else "matches",
                                size = 12f, color = Term.dim, modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 14.dp, bottom = 4.dp),
                            )
                        }
                        items(browser.results, key = { "s" + it.id }) { ItemRow(browser, it, accent) { scope.launch { if (browser.activate(it)) { focus.clearFocus(); close() } } } }
                    } else if (rows.isEmpty() && !browser.loading) {
                        item {
                            TermText(
                                if (browser.query.isEmpty()) "empty" else "no matches for \"${browser.query}\"",
                                size = 14f, color = Term.dim, modifier = Modifier.padding(20.dp),
                            )
                        }
                    }
                }
            }
        }
        browser.notice?.let {
            TermText(it, size = 13f, weight = FontWeight.Medium, color = accent, modifier = Modifier.fillMaxWidth().background(Theme.page).padding(horizontal = 20.dp, vertical = 10.dp))
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ItemRow(browser: LibraryBrowser, item: LibraryItem, accent: Color, onOpen: () -> Unit) {
    val scope = rememberCoroutineScope()
    val positions = if (item.enqueue == null) emptyList() else browser.provider.queuePositions
    var menu by remember { mutableStateOf(false) }
    Box {
        Row(item.title, item.subtitle, item.detail, item.opens != null, onLongClick = if (positions.isEmpty()) null else ({ menu = true }), onClick = onOpen)
        DropdownMenu(menu, onDismissRequest = { menu = false }, modifier = Modifier.background(Theme.page)) {
            for (p in positions) {
                DropdownMenuItem(
                    text = { TermText(p.label, color = accent) },
                    onClick = {
                        menu = false
                        scope.launch { browser.enqueue(item, p) }
                    },
                )
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Row(
    title: String,
    subtitle: String?,
    detail: String?,
    folder: Boolean,
    onLongClick: (() -> Unit)? = null,
    onClick: () -> Unit,
) {
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .background(if (pressed) Term.faint.copy(alpha = 0.35f) else Color.Transparent)
            .combinedClickable(source, null, onLongClick = onLongClick, onClick = onClick)
            .padding(horizontal = 20.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            TermText(title.lowercase() + if (folder) "/" else "", weight = if (folder) FontWeight.Medium else FontWeight.Normal)
            subtitle?.let { TermText(it.lowercase(), size = 12f, color = Term.dim) }
        }
        detail?.let { TermText(it, size = 12f, color = Term.dim) }
    }
}

/**
 * Where Spotify plays: its devices, the active one marked. Tap one to move
 * playback there. This phone is offered even while its Spotify is asleep.
 */
@Composable
private fun DevicePicker(spotify: SpotifySource, accent: Color) {
    val scope = rememberCoroutineScope()
    var devices by remember { mutableStateOf<List<SpotifySource.Device>>(emptyList()) }
    var moving by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    suspend fun refresh() {
        runCatching { spotify.devices() }.getOrNull()?.let { devices = it }
    }
    fun move(name: String, body: suspend () -> Unit) {
        scope.launch {
            error = null
            moving = name
            try {
                body()
            } catch (e: Exception) {
                error = e.message
            }
            moving = null
            refresh()
        }
    }
    LaunchedEffect(spotify.device?.id) { refresh() }
    Column(Modifier.padding(bottom = 8.dp)) {
        TermText("devices", size = 12f, color = Term.dim, modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 8.dp, bottom = 2.dp))
        @Composable
        fun row(name: String, kind: String, active: Boolean, action: suspend () -> Unit) {
            Row(
                Modifier.fillMaxWidth().heightIn(min = 44.dp).termRow(enabled = moving == null) { move(name, action) }
                    .padding(horizontal = 20.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                TermText(if (active) "> " else "  ", color = accent)
                TermText(name.lowercase(), color = if (active) accent else Term.ink, weight = if (active) FontWeight.Medium else FontWeight.Normal, modifier = Modifier.weight(1f))
                TermText(if (moving == name) "moving..." else kind, size = 12f, color = Term.dim)
            }
        }
        for (d in devices) {
            row(d.name, if (d.isPhone) "this phone" else d.type.lowercase(), d.id == spotify.device?.id) { spotify.transfer(d) }
        }
        if (devices.none { it.isPhone }) {
            row("this phone", "wakes spotify", false) { spotify.transferToThisPhone() }
        }
        error?.let { TermText("! ${it.lowercase()}", size = 12f, maxLines = 3, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp)) }
    }
}

/** What plays next. Drops from the top (where the swipe down came from). */
@Composable
fun QueuePanel(model: AppModel, accent: Color, onHeaderDrag: (Float, Float?) -> Unit, close: () -> Unit) {
    val provider = model.library?.provider
    var snapshot by remember { mutableStateOf<QueueSnapshot?>(null) }
    var error by remember { mutableStateOf<LibraryException?>(null) }
    val trackID = model.nowPlaying?.trackID
    LaunchedEffect(trackID) {
        if (provider == null) return@LaunchedEffect
        try {
            snapshot = provider.upNext()
            error = null
        } catch (e: LibraryException) {
            error = e
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            error = LibraryException.Message(e.message ?: "")
        }
    }
    Column(Modifier.fillMaxSize().swallowTaps().background(Theme.page.copy(alpha = 0.97f)).statusBarsPadding()) {
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(vertical = 6.dp).padding(bottom = 12.dp),
        ) {
            if (model.activeKind == SourceKind.SPOTIFY) {
                DevicePicker(model.spotify, accent)
                Box(Modifier.padding(horizontal = 20.dp).fillMaxWidth().height(1.dp).background(Term.faint))
            }
            val e = error
            val s = snapshot
            when {
                e != null -> {
                    TermText("! ${e.message}", size = 14f, maxLines = 4, modifier = Modifier.padding(20.dp))
                    if (e is LibraryException.NeedsLogin && provider != null) {
                        Box(Modifier.padding(horizontal = 20.dp)) { TextButtonRow("> log in to ${provider.name} again", accent) { provider.reauthorize() } }
                    }
                }
                s != null -> {
                    s.current?.let { Line("now ", it, true, accent) }
                    if (s.upcoming.isEmpty()) {
                        TermText(
                            "nothing queued. long-press a song in the library to add it.", size = 13f, color = Term.dim, maxLines = 2,
                            modifier = Modifier.padding(horizontal = 20.dp, vertical = 12.dp),
                        )
                    }
                    s.upcoming.forEachIndexed { i, entry -> Line("%2d. ".format(i + 1), entry, false, accent) }
                    s.note?.let { TermText(it, size = 12f, color = Term.dim, maxLines = 3, modifier = Modifier.padding(horizontal = 20.dp, vertical = 12.dp)) }
                }
                else -> TermText("reading queue...", size = 14f, color = Term.faint, modifier = Modifier.padding(20.dp))
            }
        }
        // The only part that moves the panel.
        Column(
            Modifier.fillMaxWidth().headerDrag(onHeaderDrag).padding(start = 20.dp, end = 20.dp, top = 10.dp, bottom = 12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                TermText("~/queue", size = 14f, weight = FontWeight.Medium, modifier = Modifier.weight(1f))
                TermText("esc", size = 13f, color = Term.dim, modifier = Modifier.termRow(onClick = close).semantics { contentDescription = "Close queue" })
            }
            Grabber()
        }
    }
}

@Composable
private fun Line(prefix: String, entry: QueueSnapshot.Entry, highlight: Boolean, accent: Color) {
    Row(Modifier.fillMaxWidth().heightIn(min = 46.dp).padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically) {
        TermText(prefix, color = if (highlight) accent else Term.dim)
        Column {
            TermText(entry.title.lowercase())
            entry.subtitle?.let { TermText(it.lowercase(), size = 12f, color = Term.dim) }
        }
    }
}
