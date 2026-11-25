package app.coverwave.ui

import android.provider.Settings
import android.view.HapticFeedbackConstants
import android.view.View
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.height
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.coverwave.core.timeText
import uniffi.cw_ffi.CoverColor

object Theme {
    /** The page behind the art: the terminal app's background. */
    val page = Color(0xFF0E0F12)
    val pageCore = CoverColor(14u, 15u, 18u)
    const val PAGE_PACKED = 0x0E0F12

    /** Accent until a cover provides one. */
    val defaultAccent = Color(196, 204, 214)
}

/**
 * The terminal look shared by every piece of chrome: monospaced glyphs on
 * the page colour, no panels, no rounded cards. The one accent comes from
 * the cover.
 */
object Term {
    val ink = Color(228, 231, 236)
    val dim = Color(128, 134, 144)
    val faint = Color(72, 76, 84)

    fun style(size: Float = 15f, weight: FontWeight = FontWeight.Normal, color: Color = ink) =
        TextStyle(fontFamily = FontFamily.Monospace, fontSize = size.sp, fontWeight = weight, color = color)

    /** Width of one glyph (monospace advances about 0.6 em). */
    fun glyphWidth(size: TextUnit) = size.value * 0.6f
}

/** Monospaced text in the terminal style. */
@Composable
fun TermText(
    text: String,
    modifier: Modifier = Modifier,
    size: Float = 15f,
    weight: FontWeight = FontWeight.Normal,
    color: Color = Term.ink,
    maxLines: Int = 1,
) {
    Text(
        text, modifier, style = Term.style(size, weight, color), maxLines = maxLines,
        softWrap = maxLines > 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
    )
}

/**
 * A tappable row that highlights on touch-down (not on release), in the
 * terminal style.
 */
fun Modifier.termRow(enabled: Boolean = true, onClick: () -> Unit): Modifier = this.then(
    Modifier.composedTermRow(enabled, onClick),
)

private fun Modifier.composedTermRow(enabled: Boolean, onClick: () -> Unit): Modifier =
    composed {
        val source = remember { MutableInteractionSource() }
        val pressed by source.collectIsPressedAsState()
        Modifier
            .background(if (pressed) Term.faint.copy(alpha = 0.35f) else Color.Transparent)
            .clickable(interactionSource = source, indication = null, enabled = enabled, onClick = onClick)
    }

/**
 * `1:42 ==========--------- 4:21`, sized to the width it is given. The
 * filled part takes the accent.
 */
@Composable
fun TermProgress(fraction: Double, elapsed: Double, duration: Double, accent: Color, modifier: Modifier = Modifier, size: Float = 13f) {
    BoxWithConstraints(modifier.height((size * 1.5f).dp)) {
        val density = LocalDensity.current
        val glyph = with(density) { (size * 0.6f).sp.toPx() }
        val widthPx = with(density) { maxWidth.toPx() }
        val left = timeText(elapsed)
        val right = if (duration > 0) timeText(duration) else "--:--"
        val n = maxOf(4, (widthPx / glyph).toInt() - left.length - right.length - 2)
        val filled = (fraction * n).toInt().coerceIn(0, n)
        Text(
            buildAnnotatedString {
                withStyle(SpanStyle(color = Term.dim)) { append("$left ") }
                withStyle(SpanStyle(color = accent)) { append("=".repeat(filled)) }
                withStyle(SpanStyle(color = Term.faint)) { append("-".repeat(n - filled)) }
                withStyle(SpanStyle(color = Term.dim)) { append(" $right") }
            },
            style = Term.style(size), maxLines = 1, softWrap = false,
        )
    }
}

/** Haptics that land on the same frame as the visual change. */
object Haptics {
    fun commit(view: View) {
        view.performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
    }

    fun tick(view: View) {
        view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK)
    }
}

/** Android's "Remove animations" (Reduce Motion's counterpart). */
@Composable
fun reduceMotion(): Boolean {
    val c = LocalContext.current
    return remember {
        Settings.Global.getFloat(c.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
}

/** Takes taps that land on a panel's empty parts, so they never reach the art below. */
fun Modifier.swallowTaps(): Modifier = composed {
    clickable(remember { MutableInteractionSource() }, indication = null) {}
}

/** A full-screen box in the page colour. */
@Composable
fun Page(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    Box(modifier.background(Theme.page)) { content() }
}
