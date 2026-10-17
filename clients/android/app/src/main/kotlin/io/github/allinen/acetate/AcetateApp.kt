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
import uniffi.acetate_ffi.CoreException
import uniffi.acetate_ffi.Editor
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
        Equalizer.init(this)
        LyricsTiming.init(this)
        Themes.init(this)
        model = Model(filesDir.path)
        model.context = this
        model.quality = Quality(this, model)
        // A network coming up may be the way to the server: it's asked at once, not at
        // the probe's next turn.
        getSystemService(android.net.ConnectivityManager::class.java).registerDefaultNetworkCallback(
            object : android.net.ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: android.net.Network) = model.core.tryNow()
            },
        )
    }
}

class Model(dataDir: String) : Listener {
    private val main = Handler(Looper.getMainLooper())

    val core: Core = Core(dataDir, this)

    var session by mutableStateOf<SessionInfo?>(core.session())
        private set
    var sync by mutableStateOf(core.syncState())
        private set

    /** Whether the server can be reached, as of the last [Change.CONNECTION]. */
    var connection by mutableStateOf(core.connection())
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
    var notice: String?
        get() = noticeText
        set(value) {
            noticeText = value
            // Kept in the core, to read again (Settings -> Earlier messages).
            if (value != null) core.note(value, false)
        }
    private var noticeText by mutableStateOf<String?>(null)

    val playback = core.playback()

    /** Set once the application exists (it needs a context). */
    lateinit var quality: Quality

    fun post(work: () -> Unit) {
        main.post(work)
    }

    /** What plays and the queue, as of the last [Change.PLAYER]. */
    var player by mutableStateOf(playback.state())
        private set

    /** The Now playing screen is open. */
    var nowPlaying by mutableStateOf(false)

    /** Edit mode is on: pages show their editing tools (only for accounts that may). */
    var editMode by mutableStateOf(false)

    /** Edit mode was on when the server went away: it comes back on with the server. */
    private var editModeWhenBack = false

    /** Bumped when the account's rights arrived (read `core.canEdit()` again). */
    var accountVersion by mutableIntStateOf(0)
        private set

    /** An edit being made (a screen over everything), if any. */
    var editor by mutableStateOf<Editor?>(null)

    /** Bumped when the edit moved (read `editor.state()` again). */
    var editorVersion by mutableIntStateOf(0)
        private set

    /** Tracks picked in a list (in the order they were picked); empty when not selecting. */
    val selected = mutableStateListOf<String>()

    fun toggleSelected(id: String) {
        if (!selected.remove(id)) selected.add(id)
    }

    /** A form (an album's, an artist's) open over everything. */
    var form by mutableStateOf<FormRef?>(null)

    /** A list being put in another order (over everything). */
    var reorder by mutableStateOf<ReorderSpec?>(null)

    /** An edit that went through, to offer undoing for a while. */
    var undoOffer by mutableStateOf<UndoOffer?>(null)

    /** Opens the editor for these tracks, or says why not. */
    fun edit(tracks: List<String>) {
        try {
            nowPlaying = false
            editor = core.editor(tracks)
        } catch (e: CoreException.Failed) {
            notice = e.reason
        }
    }

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
                Change.ACCOUNT -> {
                    accountVersion++
                    if (!core.mayEdit()) editMode = false
                }
                Change.EDITOR -> editorVersion++
                Change.CONNECTION -> {
                    val wasOffline = connection.offline
                    connection = core.connection()
                    // Editing needs the server: off while it's away (what's unsaved stays).
                    if (connection.offline && !wasOffline && editMode) {
                        editMode = false
                        editModeWhenBack = true
                        notice = "Offline: edit mode is off until the server is back."
                    } else if (!connection.offline && wasOffline && editModeWhenBack) {
                        editModeWhenBack = false
                        if (core.canEdit()) editMode = true
                    }
                }
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

    /** Reads who is logged in, and where, again (the address changed). */
    fun refreshSession() { session = core.session() }
}

/** The edit that was just saved, and whether it can be put back. */
class UndoOffer(val editor: Editor, val text: String, val canUndo: Boolean)

sealed interface Page {
    data class Album(val id: String) : Page
    data class Artist(val id: String) : Page
    data class Track(val id: String) : Page
    data class Playlist(val id: String) : Page
    data class Grouping(val id: String) : Page
    data object Groupings : Page
    data object Liked : Page
    data object Downloads : Page
    data object Changes : Page
    data object Messages : Page
    data object Settings : Page
}
