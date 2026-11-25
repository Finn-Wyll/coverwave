package app.coverwave.ui

import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import kotlin.math.abs
import kotlin.math.hypot

/** Raw touches on the art (px; velocity in px/s). */
sealed class SurfaceEvent {
    data class Tap(val at: Offset) : SurfaceEvent()
    data object TwoFingerTap : SurfaceEvent()
    data object TwoFingerSwipe : SurfaceEvent()
    data class PanChanged(val translation: Offset) : SurfaceEvent()
    data class PanEnded(val translation: Offset, val velocity: Offset) : SurfaceEvent()
    data class HoldBegan(val at: Offset) : SurfaceEvent()
    data class HoldMoved(val at: Offset) : SurfaceEvent()
    data class HoldEnded(val at: Offset) : SurfaceEvent()
}

/**
 * Recognises the art's gestures from raw pointers. A pan wins if the finger
 * moves (past the touch slop) before the hold fires at 320ms; after it
 * fires the hold tracks every movement. A second finger turns the gesture
 * into a two-finger tap, or, past 60dp sideways, a two-finger swipe.
 */
fun Modifier.surfaceGestures(density: Float, onEvent: (SurfaceEvent) -> Unit): Modifier = pointerInput(Unit) {
    val slop = viewConfiguration.touchSlop
    val holdMs = 320L
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false)
        val start = down.position
        val startTime = down.uptimeMillis
        val tracker = VelocityTracker().apply { addPosition(down.uptimeMillis, down.position) }
        var mode = 0 // 0 undecided, 1 pan, 2 hold, 3 two fingers
        var last = start
        var twoStart: Offset? = null
        var twoTravel = 0f
        var now = startTime

        while (true) {
            val event = if (mode == 0) {
                val remaining = holdMs - (now - startTime)
                withTimeoutOrNull(maxOf(1L, remaining)) { awaitPointerEvent(PointerEventPass.Main) }
            } else {
                awaitPointerEvent(PointerEventPass.Main)
            }
            if (event == null) {
                // Held still long enough.
                mode = 2
                onEvent(SurfaceEvent.HoldBegan(last))
                continue
            }
            now = event.changes.maxOf { it.uptimeMillis }
            val pressed = event.changes.filter { it.pressed }
            if (mode != 3 && pressed.size >= 2) {
                if (mode == 1) onEvent(SurfaceEvent.PanEnded(Offset.Zero, Offset.Zero))
                if (mode == 2) onEvent(SurfaceEvent.HoldEnded(last))
                mode = 3
                twoStart = centroid(pressed.map { it.position })
            }
            when (mode) {
                3 -> {
                    if (pressed.isNotEmpty()) {
                        val c = centroid(pressed.map { it.position })
                        twoTravel = maxOf(twoTravel, abs(c.x - (twoStart ?: c).x))
                    }
                    event.changes.forEach { it.consume() }
                    if (pressed.isEmpty()) {
                        if (twoTravel > 60 * density) onEvent(SurfaceEvent.TwoFingerSwipe)
                        else if (now - startTime < 600) onEvent(SurfaceEvent.TwoFingerTap)
                        return@awaitEachGesture
                    }
                }
                else -> {
                    val change = event.changes.firstOrNull { it.id == down.id } ?: event.changes.first()
                    last = change.position
                    tracker.addPosition(change.uptimeMillis, change.position)
                    if (mode == 0 && hypot(last.x - start.x, last.y - start.y) > slop) mode = 1
                    if (!change.pressed) {
                        when (mode) {
                            0 -> onEvent(SurfaceEvent.Tap(last))
                            1 -> {
                                val v = tracker.calculateVelocity()
                                onEvent(SurfaceEvent.PanEnded(last - start, Offset(v.x, v.y)))
                            }
                            2 -> onEvent(SurfaceEvent.HoldEnded(last))
                        }
                        return@awaitEachGesture
                    }
                    change.consume()
                    when (mode) {
                        1 -> onEvent(SurfaceEvent.PanChanged(last - start))
                        2 -> onEvent(SurfaceEvent.HoldMoved(last))
                    }
                }
            }
        }
    }
}

private fun centroid(points: List<Offset>): Offset =
    Offset(points.sumOf { it.x.toDouble() }.toFloat() / points.size, points.sumOf { it.y.toDouble() }.toFloat() / points.size)

/** Where a flick would come to rest (the platform's deceleration curve). */
fun projected(translation: Float, velocity: Float, rate: Float = 0.99f): Float =
    translation + (velocity / 1000f) * rate / (1 - rate)

/** Past a boundary, follow less and less. */
fun rubberband(x: Float, limit: Float, c: Float = 0.55f): Float {
    val sign = if (x < 0) -1f else 1f
    val a = abs(x)
    return sign * (a * limit * c) / (limit + c * a)
}
