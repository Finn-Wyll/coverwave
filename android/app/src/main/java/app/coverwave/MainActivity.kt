package app.coverwave

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import app.coverwave.ui.PlayerScreen

class MainActivity : ComponentActivity() {
    private val model get() = CoverwaveApp.model

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        handle(intent)
        val insets = WindowCompat.getInsetsController(window, window.decorView)
        insets.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        setContent {
            PlayerScreen(model) { immersive ->
                if (immersive) insets.hide(WindowInsetsCompat.Type.systemBars())
                else insets.show(WindowInsetsCompat.Type.systemBars())
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    override fun onResume() {
        super.onResume()
        model.sceneBecameActive()
    }

    /** Spotify's login returns to `coverwave://spotify-callback`. */
    private fun handle(intent: Intent?) {
        intent?.data?.let { model.spotify.handle(it) }
    }
}
