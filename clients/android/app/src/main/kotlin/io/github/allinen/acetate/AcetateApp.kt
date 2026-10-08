package io.github.allinen.acetate

import android.app.Application
import android.os.Handler
import android.os.Looper
import android.os.Process
import android.os.SystemClock
import android.util.Log
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.mutableStateListOf
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
        // Before the core: its first request checks a certificate.
        Tls.init(this)
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

    /** Pages opened from the library, the last on top (back pops it). */
    val pages = mutableStateListOf<Page>()

    /** Bumped when a new catalog arrived: views built before are of the old one. */
    var catalogVersion by mutableIntStateOf(0)
        private set

    /** Bumped when what plays or the queue changed (read `playback.state()` again). */
    var playerVersion by mutableIntStateOf(0)
        private set

    /** Bumped when a star or rating changed (read `core.mark(id)` again). */
    var marksVersion by mutableIntStateOf(0)
        private set

    /** Something to tell the user once (a change that failed); the screen clears it. */
    var notice by mutableStateOf<String?>(null)

    val playback = core.playback()

    /** What plays and the queue, as of the last [Change.PLAYER]. */
    var player by mutableStateOf(playback.state())
        private set

    /** The Now playing screen is open. */
    var nowPlaying by mutableStateOf(false)

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
                Change.PLAYER -> {
                    player = playback.state()
                    playerVersion++
                }
                Change.MARKS -> marksVersion++
                Change.CATALOG -> {
                    catalogVersion++
                    // For measuring (adb logcat -s Acetate): how long from the process starting.
                    Log.i("Acetate", "catalog ${core.syncState().tracks} tracks at ${SystemClock.elapsedRealtime() - Process.getStartElapsedRealtime()} ms")
                }
                Change.SYNC -> core.syncState().let {
                    Log.i("Acetate", "sync: refreshing=${it.refreshing} have=${it.haveCatalog} error=${it.error}")
                }
            }
            sync = core.syncState()
        }
    }

    override fun problem(message: String) {
        main.post { notice = message }
    }

    fun logout() = core.logout()
}

sealed interface Page {
    data class Album(val id: String) : Page
    data class Artist(val id: String) : Page
}
