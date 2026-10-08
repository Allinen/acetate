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
import uniffi.acetate_ffi.LinkTarget
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
        Output.init(this)
        LyricsTiming.init(this)
        model = Model(filesDir.path)
        model.context = this
        model.quality = Quality(this, model)
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

    /** Which library tab is shown: Albums, Artists, Tracks, Playlists. */
    var tab by mutableIntStateOf(0)

    /** What's typed on the Search tab (kept when going to another tab). */
    var searchText by mutableStateOf("")

    /** Bumped when the playlists (or one's entries) changed. */
    var playlistsVersion by mutableIntStateOf(0)
        private set

    /** Bumped when a track's details (lyrics…) arrived. */
    var detailsVersion by mutableIntStateOf(0)
        private set

    /** A video being watched (full screen), if any. */
    var video by mutableStateOf<VideoRef?>(null)

    /** Bumped when downloads moved (a file came, was taken away, the queue changed). */
    var downloadsVersion by mutableIntStateOf(0)
        private set

    /** What's kept on the device, as of the last [Change.DOWNLOADS]. */
    var downloads by mutableStateOf(core.downloads())

    /** How fast downloads are coming now, in bytes per second (read while they run). */
    var downloadSpeed by mutableStateOf(0L)

    /** The app's context, set at start (the download service needs it). */
    lateinit var context: android.content.Context

    /** Bumped when a star or rating changed (read `core.mark(id)` again). */
    var marksVersion by mutableIntStateOf(0)
        private set

    /** Something to tell the user once (a change that failed); the screen clears it. */
    var notice by mutableStateOf<String?>(null)

    val playback = core.playback()

    /** Set once the application exists (it needs a context). */
    lateinit var quality: Quality

    fun post(work: () -> Unit) {
        main.post(work)
    }

    /** What plays and the queue, as of the last [Change.PLAYER]. */
    var player by mutableStateOf(playback.state())
        private set

    /** The Settings dialog is open. */
    var settingsOpen by mutableStateOf(false)

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
                Change.MARKS -> {
                    marksVersion++
                    // The liked songs may be kept on the device.
                    core.refreshDownloads()
                }
                Change.PLAYLISTS -> {
                    playlistsVersion++
                    core.refreshDownloads()
                }
                Change.DOWNLOADS -> {
                    downloadsVersion++
                    downloads = core.downloads()
                    if (downloads.queued > 0uL && !downloads.paused) DownloadService.start(context)
                }
                Change.DETAILS -> detailsVersion++
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
            if (pendingLink != null) openLink(null)
        }
    }

    override fun notice(message: String) {
        main.post { notice = message }
    }

    /** A link to open once there's a library to open it in. */
    private var pendingLink: String? = null

    /** Opens an `acetate://open/…` link (now, or when the library is there). */
    fun openLink(text: String?) {
        if (text != null) pendingLink = text
        val link = pendingLink ?: return
        if (session == null || !sync.haveCatalog) return
        pendingLink = null
        when (val target = core.resolveLink(link)) {
            is LinkTarget.Album -> { pages.clear(); pages.add(Page.Album(target.id)); nowPlaying = false }
            is LinkTarget.Artist -> { pages.clear(); pages.add(Page.Artist(target.id)); nowPlaying = false }
            is LinkTarget.OtherServer -> notice = "That link is to another server (${target.server}); you're logged in to ${session?.server}."
            LinkTarget.NotThere -> notice = "That isn't in your library (yet)."
            LinkTarget.Invalid -> notice = "That isn't a link this app opens."
        }
    }

    fun logout() = core.logout()
}

sealed interface Page {
    data class Album(val id: String) : Page
    data class Artist(val id: String) : Page
    data class Track(val id: String) : Page
    data class Playlist(val id: String) : Page
    data object Liked : Page
    data object Downloads : Page
}
