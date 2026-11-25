package app.coverwave.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.scale
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import app.coverwave.AppModel
import app.coverwave.GestureScheme
import app.coverwave.core.CoverCore
import app.coverwave.core.LyricsStatus
import app.coverwave.core.timeText
import app.coverwave.sources.NowPlaying
import app.coverwave.sources.SourceKind
import app.coverwave.sources.SourceStatus
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.roundToInt

/** A spring that settles in about [response] seconds, without overshoot. */
fun <T> responseSpring(response: Float = 0.35f, damping: Float = 1f) =
    spring<T>(dampingRatio = damping, stiffness = (2 * Math.PI / response).let { (it * it).toFloat() })

/**
 * The one screen: the cover, and nothing else until you touch it. The art
 * is the control (see [GestureScheme]); text appears in the cover's own
 * terminal style while you touch, and the library and queue slide over it
 * following the finger.
 */
@Composable
fun PlayerScreen(model: AppModel, onImmersive: (Boolean) -> Unit) {
    val reduce = reduceMotion()
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    val density = LocalDensity.current.density
    val accent = model.accent

    var showSettings by remember { mutableStateOf(false) }
    var hudVisible by remember { mutableStateOf(true) }
    var status by remember { mutableStateOf<String?>(null) }
    var hideJob by remember { mutableStateOf<Job?>(null) }
    var statusJob by remember { mutableStateOf<Job?>(null) }

    // Live gesture state.
    var axis by remember { mutableStateOf(0) } // 0 none, 1 horizontal, 2 vertical
    val artOffset = remember { Animatable(0f) }
    var scrubStart by remember { mutableStateOf<Pair<Double, Float>?>(null) }
    var scrub by remember { mutableStateOf<Double?>(null) }
    var dialCenter by remember { mutableStateOf<Offset?>(null) }
    var dialPick by remember { mutableStateOf<DialOption?>(null) }

    // Panels: open state plus their offset from the resting place.
    var libraryOpen by remember { mutableStateOf(false) }
    var queueOpen by remember { mutableStateOf(false) }
    val libraryY = remember { Animatable(Float.MAX_VALUE) }
    val queueY = remember { Animatable(-Float.MAX_VALUE) }
    var screenH by remember { mutableStateOf(1f) }
    var screenW by remember { mutableStateOf(1f) }
    val libraryH = screenH * 0.86f
    val queueH = screenH * 0.58f
    val swipeCommit = 90 * density

    fun flash(text: String?, hold: Boolean = false) {
        statusJob?.cancel()
        status = text
        if (text == null || hold) return
        statusJob = scope.launch {
            delay(1600)
            status = null
        }
    }

    /** Shows the HUD, then fades it while music plays. */
    fun poke() {
        hudVisible = true
        hideJob?.cancel()
        hideJob = scope.launch {
            delay(3500)
            if (model.nowPlaying?.isPlaying == true && scrub == null && dialCenter == null) hudVisible = false
        }
    }

    fun spec(velocity: Float, distance: Float) =
        if (reduce) tween<Float>(200) else responseSpring(0.35f)

    fun setLibrary(open: Boolean, velocity: Float = 0f) {
        if (open && queueOpen) {
            queueOpen = false
            scope.launch { queueY.animateTo(-(queueH + 40), responseSpring()) }
        }
        if (open != libraryOpen) Haptics.commit(view)
        libraryOpen = open
        val to = if (open) 0f else libraryH + 40
        scope.launch {
            if (libraryY.value == Float.MAX_VALUE) libraryY.snapTo(libraryH + 40)
            libraryY.animateTo(to, spec(velocity, to - libraryY.value), velocity)
        }
    }

    fun setQueue(open: Boolean, velocity: Float = 0f) {
        if (open && libraryOpen) {
            libraryOpen = false
            scope.launch { libraryY.animateTo(libraryH + 40, responseSpring()) }
        }
        if (open != queueOpen) Haptics.commit(view)
        queueOpen = open
        val to = if (open) 0f else -(queueH + 40)
        scope.launch {
            if (queueY.value == -Float.MAX_VALUE) queueY.snapTo(-(queueH + 40))
            queueY.animateTo(to, spec(velocity, to - queueY.value), velocity)
        }
    }

    fun playPause() {
        val source = model.active ?: return
        val playing = model.nowPlaying?.isPlaying ?: false
        source.playPause()
        Haptics.commit(view)
        flash(if (playing) "|| paused" else "> playing")
    }

    fun next() {
        model.active?.next()
        Haptics.commit(view)
        flash(">> next")
    }

    fun previous() {
        model.active?.previous()
        Haptics.commit(view)
        flash("<< previous")
    }

    fun finishScrub(fraction: Double) {
        val np = model.nowPlaying
        val d = np?.duration
        if (d != null && d > 0) {
            model.active?.seek(fraction * d)
            Haptics.commit(view)
            flash("> ${timeText(fraction * d)}")
        }
        // Hold the new position until the source reports it.
        scope.launch {
            delay(600)
            scrub = null
        }
    }

    fun toggleLyrics() {
        model.settings.showLyrics = !model.settings.showLyrics
        Haptics.commit(view)
        if (!model.settings.showLyrics) flash("cover")
        else flash(
            when (model.lyricsStatus) {
                LyricsStatus.FOUND -> "lyrics"
                LyricsStatus.SEARCHING -> "looking up lyrics"
                LyricsStatus.INSTRUMENTAL -> "instrumental, no lyrics"
                else -> "no lyrics found for this song"
            },
        )
    }

    fun handle(e: SurfaceEvent) {
        poke()
        val scheme = model.settings.gestures
        when (e) {
            is SurfaceEvent.Tap -> {
                if (!model.settings.seenGestureGuide) return
                when (scheme) {
                    GestureScheme.DIRECT, GestureScheme.DIAL -> playPause()
                    GestureScheme.ZONES -> {
                        val third = e.at.x / maxOf(1f, screenW)
                        if (third < 1f / 3) previous() else if (third > 2f / 3) next() else playPause()
                    }
                }
            }
            SurfaceEvent.TwoFingerTap -> showSettings = true
            SurfaceEvent.TwoFingerSwipe -> toggleLyrics()
            is SurfaceEvent.PanChanged -> {
                val t = e.translation
                if (axis == 0) axis = if (abs(t.x) > abs(t.y)) 1 else 2
                if (axis == 1) {
                    scope.launch { artOffset.snapTo(rubberband(t.x, 60 * density)) }
                    flash(if (t.x < -swipeCommit) "> next" else if (t.x > swipeCommit) "< previous" else null, hold = true)
                } else if (t.y < 0) {
                    // The panel follows the finger 1:1 from its edge.
                    scope.launch { libraryY.snapTo(libraryH + 40 + t.y) }
                } else {
                    scope.launch { queueY.snapTo(-(queueH + 40) + t.y) }
                }
            }
            is SurfaceEvent.PanEnded -> {
                val (t, v) = e.translation to e.velocity
                if (axis == 1) {
                    val p = projected(t.x, v.x)
                    if (p < -swipeCommit) next() else if (p > swipeCommit) previous() else flash(null)
                    scope.launch { artOffset.animateTo(0f, if (reduce) tween(200) else responseSpring(), v.x) }
                } else if (axis == 2) {
                    val p = projected(t.y, v.y)
                    if (t.y < 0) setLibrary(p < -swipeCommit * 1.4f, v.y) else setQueue(p > swipeCommit * 1.4f, v.y)
                }
                axis = 0
            }
            is SurfaceEvent.HoldBegan -> {
                Haptics.tick(view)
                if (scheme == GestureScheme.DIAL) {
                    dialCenter = e.at
                    dialPick = null
                } else {
                    val np = model.nowPlaying
                    val d = np?.duration
                    if (np != null && d != null && d > 0) {
                        val f = minOf(1.0, np.positionAt() / d)
                        scrubStart = f to e.at.x
                        scrub = f
                    }
                }
            }
            is SurfaceEvent.HoldMoved -> {
                val c = dialCenter
                val start = scrubStart
                if (c != null) {
                    val pick = DialOption.at((e.at.x - c.x) / density, (e.at.y - c.y) / density)
                    if (pick != dialPick) {
                        dialPick = pick
                        if (pick != null) Haptics.tick(view)
                    }
                } else if (start != null) {
                    // A full screen width spans the whole track.
                    scrub = (start.first + (e.at.x - start.second) / maxOf(1f, screenW)).coerceIn(0.0, 1.0)
                }
            }
            is SurfaceEvent.HoldEnded -> {
                if (dialCenter != null) {
                    val pick = dialPick
                    dialCenter = null
                    dialPick = null
                    when (pick) {
                        DialOption.LIBRARY -> setLibrary(true)
                        DialOption.QUEUE -> setQueue(true)
                        DialOption.PREVIOUS -> previous()
                        DialOption.NEXT -> next()
                        null -> {}
                    }
                } else scrub?.let { finishScrub(it) }
                scrubStart = null
            }
        }
    }

    val np = model.nowPlaying
    LaunchedEffect(np?.trackID, np?.isPlaying) { poke() }
    LaunchedEffect(hudVisible, libraryOpen) { onImmersive(!hudVisible || libraryOpen) }
    BackHandler(libraryOpen || queueOpen || showSettings) {
        when {
            showSettings -> showSettings = false
            libraryOpen -> setLibrary(false)
            queueOpen -> setQueue(false)
        }
    }

    Box(
        Modifier
            .fillMaxSize()
            .background(Theme.page)
            .onSizeChanged {
                screenH = it.height.toFloat()
                screenW = it.width.toFloat()
            },
    ) {
        // The art, dimmed behind the scrub time, the dial and open panels.
        val animate = model.settings.animates(reduce)
        Crossfade(model.coverHash, animationSpec = tween(450), label = "cover") { _ ->
            Box(Modifier.fillMaxSize().graphicsLayer { translationX = if (reduce) 0f else artOffset.value }) {
                CoverCanvas(model, animate)
            }
        }
        val libDim = if (libraryOpen) 1f else (1 - (libraryY.value.coerceAtMost(libraryH + 40) / (libraryH + 40))).coerceIn(0f, 1f)
        val qDim = if (queueOpen) 1f else (1 - (-queueY.value.coerceAtLeast(-(queueH + 40)) / (queueH + 40))).coerceIn(0f, 1f)
        val dim = if (scrub != null || dialCenter != null) 0.45f else 0.5f * maxOf(libDim, qDim)
        if (dim > 0) Box(Modifier.fillMaxSize().background(Theme.page.copy(alpha = dim)))

        if (model.isFollowing) {
            Box(
                Modifier
                    .fillMaxSize()
                    .surfaceGestures(density, ::handle)
                    .semantics {
                        contentDescription = np?.let { "${it.title} by ${it.artist}, ${if (it.isPlaying) "playing" else "paused"}" }
                            ?: "Cover. Nothing playing"
                        onClick("Play or pause") { playPause(); true }
                        customActions = listOf(
                            CustomAccessibilityAction("Next") { next(); true },
                            CustomAccessibilityAction("Previous") { previous(); true },
                            CustomAccessibilityAction("Library") { setLibrary(true); true },
                            CustomAccessibilityAction("Queue") { setQueue(true); true },
                            CustomAccessibilityAction("Settings") { showSettings = true; true },
                        )
                    },
            )

            dialCenter?.let { c ->
                DialView(dialPick, accent, Modifier.offset { IntOffset(c.x.roundToInt(), c.y.roundToInt()) })
            }

            val hudAlpha = if (hudVisible || scrub != null) 1f else 0f
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomStart) {
                Box(Modifier.alpha(animatedAlpha(hudAlpha))) {
                    HUD(
                        track = np,
                        sourceName = model.activeKind?.label ?: "",
                        elsewhere = if (model.activeKind == SourceKind.SPOTIFY) model.spotify.device?.takeIf { !it.isPhone }?.name
                        else if (model.activeKind == SourceKind.APPLE_MUSIC) model.appleMusic.playerName?.takeIf { it != "Apple Music" }
                        else null,
                        elsewhereIsApp = model.activeKind == SourceKind.APPLE_MUSIC,
                        status = status ?: modelStatus(model),
                        scrub = scrub,
                        accent = accent,
                        onScrub = { f, done ->
                            poke()
                            if (done) finishScrub(f) else scrub = f
                        },
                        enabled = hudVisible,
                    )
                }
            }

            // Tapping the art around an open panel closes it.
            if (libraryOpen || queueOpen) {
                Box(
                    Modifier.fillMaxSize().clickable(remember { MutableInteractionSource() }, null) {
                        setLibrary(false)
                        setQueue(false)
                    },
                )
            }

            // Queue: drops from the top.
            if (queueOpen || queueY.value > -(queueH + 40) && queueY.value != -Float.MAX_VALUE) {
                Box(
                    Modifier.fillMaxWidth().height(with(LocalDensity.current) { queueH.toDp() })
                        .graphicsLayer { translationY = if (queueY.value > 0) rubberband(queueY.value, queueH) else queueY.value },
                ) {
                    QueuePanel(
                        model = model,
                        accent = accent,
                        onHeaderDrag = { dy, velocity ->
                            if (velocity != null) {
                                val close = projected(dy, velocity) < -queueH * 0.3f
                                setQueue(!close, velocity)
                            } else scope.launch { queueY.snapTo(dy) }
                        },
                        close = { setQueue(false) },
                    )
                }
            }

            // Library: rises from the bottom.
            val browser = model.library
            if (browser != null && (libraryOpen || libraryY.value < libraryH + 40)) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomStart) {
                    Box(
                        Modifier.fillMaxWidth().height(with(LocalDensity.current) { libraryH.toDp() })
                            .graphicsLayer { translationY = if (libraryY.value < 0) rubberband(libraryY.value, libraryH) else libraryY.value },
                    ) {
                        LibraryPanel(
                            browser = browser,
                            accent = accent,
                            onHeaderDrag = { dy, velocity ->
                                if (velocity != null) {
                                    val close = projected(dy, velocity) > libraryH * 0.3f
                                    setLibrary(!close, velocity)
                                } else scope.launch { libraryY.snapTo(dy) }
                            },
                            close = { setLibrary(false) },
                        )
                    }
                }
            }

            if (!model.settings.seenGestureGuide) {
                GestureGuide(model.settings.gestures, accent) { model.settings.seenGestureGuide = true }
            }
        } else {
            ConnectScreen(model, accent) { showSettings = true }
        }

        AnimatedVisibility(showSettings, enter = fadeIn(tween(200)), exit = fadeOut(tween(200))) {
            SettingsScreen(model, accent, onClose = { showSettings = false }, onShowGuide = {
                model.settings.seenGestureGuide = false
                showSettings = false
            })
        }
    }
}

@Composable
private fun animatedAlpha(target: Float): Float {
    val a = remember { Animatable(target) }
    LaunchedEffect(target) { a.animateTo(target, tween(if (target > 0) 200 else 600)) }
    return a.value
}

private fun modelStatus(model: AppModel): String? = when (val s = model.core.modelState) {
    is CoverCore.ModelState.Downloading -> "preparing covers ${(s.fraction * 100).toInt()}%"
    CoverCore.ModelState.Loading -> "preparing covers"
    else -> if (model.nowPlaying?.isPlaying == false) "|| paused" else null
}

/**
 * Track, artist and an ASCII progress bar, bottom-left over a scrim of the
 * page colour. It shows while you touch, while paused, and briefly after
 * anything changes; the art is otherwise left alone.
 */
@Composable
fun HUD(
    track: NowPlaying?,
    sourceName: String,
    elsewhere: String?,
    elsewhereIsApp: Boolean,
    status: String?,
    scrub: Double?,
    accent: Color,
    onScrub: (Double, Boolean) -> Unit,
    enabled: Boolean,
) {
    var now by remember { mutableStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(500)
            now = System.currentTimeMillis()
        }
    }
    Column(
        Modifier
            .fillMaxWidth()
            .background(Brush.verticalGradient(listOf(Theme.page.copy(alpha = 0f), Theme.page.copy(alpha = 0.88f))))
            .navigationBarsPadding()
            .padding(start = 20.dp, end = 20.dp, top = 48.dp, bottom = 12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        status?.let { TermText(it, size = 13f, weight = FontWeight.Medium, color = accent) }
        if (track != null) {
            val duration = track.duration ?: 0.0
            val live = if (duration > 0) minOf(1.0, track.positionAt(now) / duration) else 0.0
            val shown = scrub ?: live
            TermText(track.title, size = 18f, weight = FontWeight.SemiBold)
            TermText(listOf(track.artist, track.album).filter { it.isNotEmpty() }.joinToString(" / "), size = 13f, color = Term.dim)
            elsewhere?.let {
                TermText(
                    if (elsewhereIsApp) "from ${it.lowercase()}" else "on ${it.lowercase()}, swipe down to change",
                    size = 12f, color = accent,
                )
            }
            if (scrub != null) {
                TermText(timeText(shown * duration), size = 40f, weight = FontWeight.Light, modifier = Modifier.padding(top = 4.dp))
            }
            var barWidth by remember { mutableStateOf(1f) }
            Box(
                Modifier
                    .fillMaxWidth()
                    .padding(top = 2.dp)
                    .onSizeChanged { barWidth = it.width.toFloat() }
                    .then(
                        if (!enabled) Modifier
                        else Modifier.pointerInput(Unit) {
                            // The bar itself can be dragged in any scheme.
                            awaitEachGesture {
                                val down = awaitFirstDown()
                                fun f(x: Float) = (x / barWidth).toDouble().coerceIn(0.0, 1.0)
                                onScrub(f(down.position.x), false)
                                while (true) {
                                    val e = awaitPointerEvent()
                                    val c = e.changes.firstOrNull { it.id == down.id } ?: break
                                    c.consume()
                                    if (!c.pressed) {
                                        onScrub(f(c.position.x), true)
                                        break
                                    }
                                    onScrub(f(c.position.x), false)
                                }
                            }
                        },
                    )
                    .semantics { contentDescription = "Position ${timeText(shown * duration)} of ${timeText(duration)}" },
            ) {
                TermProgress(shown, shown * duration, duration, accent)
            }
        } else {
            TermText("nothing playing", size = 18f, weight = FontWeight.SemiBold)
            TermText("start something in $sourceName, or swipe up for your library", size = 13f, color = Term.dim)
        }
    }
}

/** The dial's options, by direction from the press (dp). */
enum class DialOption(val label: String, val dx: Float, val dy: Float) {
    LIBRARY("library", 0f, -86f),
    QUEUE("queue", 0f, 86f),
    PREVIOUS("prev", -86f, 0f),
    NEXT("next", 86f, 0f);

    companion object {
        /** The option a finger (dx, dy) dp away points at (null in the dead zone). */
        fun at(dx: Float, dy: Float): DialOption? {
            if (hypot(dx, dy) <= 34) return null
            return if (abs(dx) > abs(dy)) (if (dx < 0) PREVIOUS else NEXT) else (if (dy < 0) LIBRARY else QUEUE)
        }
    }
}

/** Four labels around the finger, in glyphs; the one pointed at is bracketed in the accent. */
@Composable
fun DialView(selected: DialOption?, accent: Color, modifier: Modifier) {
    Box(modifier) {
        TermText("+", size = 20f, color = Term.dim, modifier = Modifier.offset((-6).dp, (-14).dp))
        for (o in DialOption.entries) {
            val on = o == selected
            val label = if (on) "[${o.label}]" else o.label
            val w = label.length * 9.6f
            TermText(
                label, size = 15f, weight = if (on) FontWeight.SemiBold else FontWeight.Normal,
                color = if (on) accent else Term.ink,
                modifier = Modifier
                    .offset((o.dx - w / 2).dp, (o.dy - 12).dp)
                    .scale(if (on) 1.12f else 1f)
                    .background(Theme.page.copy(alpha = 0.82f))
                    .padding(horizontal = 6.dp, vertical = 3.dp),
            )
        }
    }
}

/** The cheat sheet for the chosen scheme, shown once and from Settings. */
@Composable
fun GestureGuide(scheme: GestureScheme, accent: Color, dismiss: () -> Unit) {
    Column(
        Modifier
            .fillMaxSize()
            .background(Theme.page.copy(alpha = 0.94f))
            .clickable(remember { MutableInteractionSource() }, null, onClick = dismiss)
            .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(18.dp, Alignment.CenterVertically),
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            TermText("gestures: ${scheme.label.lowercase()}", size = 18f, weight = FontWeight.SemiBold)
            TermText("the art is the control. change the scheme in settings.", size = 13f, color = Term.dim, maxLines = 2)
        }
        GuideTable(scheme)
        TermText("tap anywhere to start", size = 13f, weight = FontWeight.Medium, color = accent)
    }
}

@Composable
fun GuideTable(scheme: GestureScheme) {
    val lines = scheme.guide
    val width = (lines.maxOfOrNull { it.first.length } ?: 0) + 2
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        for ((g, a) in lines) {
            Row {
                TermText(g.padEnd(width), size = 14f, color = Term.dim)
                TermText(a, size = 14f)
            }
        }
    }
}

/** Before a source is connected: pick one, in the same terminal style. */
@Composable
fun ConnectScreen(model: AppModel, accent: Color, openSettings: () -> Unit) {
    BoxWithConstraints(Modifier.fillMaxSize()) {
        Column(
            Modifier
                .fillMaxSize()
                .background(Brush.verticalGradient(0.4f to Theme.page.copy(alpha = 0f), 0.75f to Theme.page.copy(alpha = 0.9f)))
                .navigationBarsPadding()
                .padding(horizontal = 24.dp)
                .padding(bottom = 32.dp)
                .widthIn(max = 560.dp),
            verticalArrangement = Arrangement.spacedBy(20.dp, Alignment.Bottom),
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                TermText("coverwave", size = 22f, weight = FontWeight.SemiBold)
                TermText("follow your music. the cover moves with it.", size = 13f, color = Term.dim)
            }
            Column {
                for (kind in SourceKind.entries) {
                    val source = model.source(kind)
                    val busy = model.activeKind == kind && source.status == SourceStatus.Connecting
                    val disabled = kind != SourceKind.APPLE_MUSIC && !model.spotify.isConfigured
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .heightIn(min = 52.dp)
                            .termRow(enabled = !busy && !disabled) { model.choose(kind) },
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column(Modifier.weight(1f)) {
                            TermText("> ${kind.label.lowercase()}", size = 17f, weight = FontWeight.Medium, color = if (disabled) Term.faint else Term.ink)
                            TermText("  ${kind.blurb}", size = 12f, color = Term.dim)
                        }
                        if (busy) {
                            TermText(if (kind == SourceKind.OFFLINE) "waiting for spotify..." else "connecting...", size = 12f, color = accent)
                        }
                    }
                }
            }
            val kind = model.activeKind
            val msg = kind?.let { model.source(it).status }?.takeIf { it != SourceStatus.Connecting }?.message
                ?: if (!model.spotify.isConfigured) "spotify needs your app's client id: add it in settings" else null
            msg?.let { TermText("! ${it.lowercase()}", size = 12f, color = Term.dim, maxLines = 3) }
            TermText(
                "> settings", size = 14f, weight = FontWeight.Medium, color = accent,
                modifier = Modifier.termRow(onClick = openSettings).padding(vertical = 8.dp),
            )
        }
    }
}
