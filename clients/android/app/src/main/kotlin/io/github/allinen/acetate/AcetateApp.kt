package io.github.allinen.acetate

import android.app.Application
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import uniffi.acetate_ffi.Change
import uniffi.acetate_ffi.Core
import uniffi.acetate_ffi.Listener
import uniffi.acetate_ffi.SessionInfo
import uniffi.acetate_ffi.SyncState

/** The one core of the process, and what the screens read of it (Compose state). */
class AcetateApp : Application() {
    lateinit var model: Model
        private set

    override fun onCreate() {
        super.onCreate()
        model = Model(filesDir.path)
    }
}

class Model(dataDir: String) : Listener {
    private val main = Handler(Looper.getMainLooper())

    val core: Core = Core(dataDir, this)

    var session by mutableStateOf<SessionInfo?>(core.session())
        private set
    var sync by mutableStateOf(core.syncState())
        private set

    /** Bumped when a new catalog arrived: views built before are of the old one. */
    var catalogVersion by mutableIntStateOf(0)
        private set

    init {
        if (session != null) core.startSync()
    }

    // Called from any thread.
    override fun changed(what: Change) {
        main.post {
            when (what) {
                Change.SESSION -> {
                    session = core.session()
                    if (session != null) core.startSync()
                }
                Change.CATALOG -> catalogVersion++
                Change.SYNC -> {}
            }
            sync = core.syncState()
        }
    }

    fun logout() = core.logout()
}
