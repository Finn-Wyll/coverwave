package app.coverwave

import android.app.Application
import app.coverwave.core.NetworkMonitor
import app.coverwave.core.Platform

/** Holds the one [AppModel] for the app's lifetime. */
class CoverwaveApp : Application() {
    companion object {
        lateinit var model: AppModel
            private set
    }

    override fun onCreate() {
        super.onCreate()
        Platform.init(this)
        NetworkMonitor.start(this)
        model = AppModel()
        model.start()
    }
}
