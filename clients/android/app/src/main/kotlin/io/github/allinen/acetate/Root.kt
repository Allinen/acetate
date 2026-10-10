package io.github.allinen.acetate

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.ui.Modifier
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Text
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.getValue
import androidx.compose.material3.TextButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.background

@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
fun Root(model: Model) {
    if (model.session == null) {
        LoginScreen(model)
        return
    }
    BackHandler(enabled = model.pages.isNotEmpty() && !model.nowPlaying) { model.pages.removeAt(model.pages.lastIndex) }
    // The bottom bar's inset is taken here once, so the screens above the mini player
    // don't leave a gap of it.
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().imePadding()) {
            Box(Modifier.weight(1f)) {
                when (val page = model.pages.lastOrNull()) {
                    null -> LibraryScreen(model)
                    is Page.Album -> AlbumScreen(model, page.id)
                    is Page.Artist -> ArtistScreen(model, page.id)
                    is Page.Track -> TrackScreen(model, page.id)
                    is Page.Playlist -> PlaylistScreen(model, page.id)
                    Page.Liked -> LikedScreen(model)
                    Page.Downloads -> DownloadsScreen(model)
                    Page.Changes -> ChangesScreen(model)
                }
            }
            // The keyboard takes the place of the bars while typing.
            if (!WindowInsets.isImeVisible) {
                if (model.connection.offline) OfflineBar(model)
                if (model.player.current != null) MiniPlayer(model)
                BottomBar(model)
            }
        }
        val snacks = remember { SnackbarHostState() }
        LaunchedEffect(model.notice) {
            model.notice?.let {
                snacks.showSnackbar(it)
                model.notice = null
            }
        }
        // An edit just saved: Undo, for a while.
        LaunchedEffect(model.undoOffer) {
            val offer = model.undoOffer ?: return@LaunchedEffect
            val result = snacks.showSnackbar(offer.text, if (offer.canUndo) "Undo" else null, duration = androidx.compose.material3.SnackbarDuration.Long)
            // (Cleared only after the undo: changing it ends this effect, and the undo with it.)
            if (result == androidx.compose.material3.SnackbarResult.ActionPerformed) {
                try {
                    offer.editor.undo()
                    model.notice = "Undone."
                    model.core.startSync()
                } catch (e: uniffi.acetate_ffi.CoreException) {
                    model.notice = "Couldn't undo it: ${e.message}"
                }
            }
            if (model.undoOffer === offer) model.undoOffer = null
        }
        SnackbarHost(snacks, Modifier.align(Alignment.BottomCenter).windowInsetsPadding(WindowInsets.navigationBars).padding(bottom = 140.dp))
        AnimatedVisibility(model.nowPlaying, enter = slideInVertically { it }, exit = slideOutVertically { it }) {
            NowPlaying(model)
        }
        // An edit is above Now playing too.
        model.editor?.let { EditorScreen(model, it) }
        when (val form = model.form) {
            is FormRef.Album -> AlbumFormScreen(model, form.id)
            is FormRef.Artist -> ArtistFormScreen(model, form.id)
            is FormRef.Covers -> CoversScreen(model, form.album)
            is FormRef.Lyrics -> LyricsFormScreen(model, form.track)
            null -> {}
        }
        model.reorder?.let { ReorderScreen(model, it) }
        // Last, so it's above everything (Now playing included).
        model.video?.let { VideoScreen(model, it) }
    }
}

private val tabs = listOf("Albums", "Artists", "Tracks", "Playlists", "Search")

/** Where to go in the library; tapping the one you're on goes back to its top. */
@Composable
private fun BottomBar(model: Model) {
    NavigationBar(containerColor = Palette.sidebar) {
        tabs.forEachIndexed { i, name ->
            NavigationBarItem(
                selected = model.tab == i && model.pages.isEmpty(),
                onClick = {
                    model.tab = i
                    model.pages.clear()
                    model.nowPlaying = false
                },
                icon = { TabIcon(i, if (model.tab == i && model.pages.isEmpty()) Palette.accent else Palette.muted) },
                label = { Text(name) },
                colors = NavigationBarItemDefaults.colors(
                    selectedTextColor = Palette.accent, unselectedTextColor = Palette.muted,
                    indicatorColor = Palette.accentSoft,
                ),
            )
        }
    }
}

/** A small drawn icon for each tab: a disc, a person, a note, a list. */
@Composable
private fun TabIcon(tab: Int, color: Color) {
    Canvas(Modifier.size(24.dp)) {
        val w = size.width
        val line = Stroke(width = w * 0.09f, cap = StrokeCap.Round)
        when (tab) {
            0 -> {
                drawCircle(color, w * 0.42f, style = line)
                drawCircle(color, w * 0.1f)
            }
            1 -> {
                drawCircle(color, w * 0.18f, Offset(w / 2, w * 0.32f))
                drawArc(color, 180f, 180f, false, Offset(w * 0.16f, w * 0.56f), Size(w * 0.68f, w * 0.7f))
            }
            2 -> {
                drawCircle(color, w * 0.16f, Offset(w * 0.35f, w * 0.75f))
                drawLine(color, Offset(w * 0.5f, w * 0.75f), Offset(w * 0.5f, w * 0.15f), line.width, StrokeCap.Round)
                drawLine(color, Offset(w * 0.5f, w * 0.15f), Offset(w * 0.8f, w * 0.3f), line.width, StrokeCap.Round)
            }
            4 -> {
                drawCircle(color, w * 0.28f, Offset(w * 0.43f, w * 0.43f), style = line)
                drawLine(color, Offset(w * 0.63f, w * 0.63f), Offset(w * 0.87f, w * 0.87f), line.width, StrokeCap.Round)
            }
            else -> for (i in 0..2) {
                drawLine(color, Offset(w * 0.15f, w * (0.25f + i * 0.25f)), Offset(w * 0.85f, w * (0.25f + i * 0.25f)), line.width, StrokeCap.Round)
            }
        }
    }
}

/** The server can't be reached: for how long, and a way to ask it now. */
@Composable
private fun OfflineBar(model: Model) {
    // Read again every half minute, for the "for how long".
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            kotlinx.coroutines.delay(30_000)
            now = System.currentTimeMillis()
        }
    }
    val minutes = model.connection.sinceMs?.let { (now - it.toLong()) / 60_000 } ?: 0
    val text = when {
        minutes < 2 -> "Offline"
        minutes < 90 -> "Offline for $minutes minutes"
        minutes < 48 * 60 -> "Offline for ${(minutes + 30) / 60} hours"
        else -> "Offline for ${minutes / (24 * 60)} days"
    }
    Row(
        Modifier.fillMaxWidth().background(Palette.sidebar).padding(16.dp, 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val waiting = model.connection.waiting.toLong()
        Column(Modifier.weight(1f)) {
            Text(text, color = Palette.danger, style = MaterialTheme.typography.bodySmall)
            // Stars, ratings, plays and playlist additions made offline: sent when it's back.
            if (waiting > 0) {
                Text(
                    if (waiting == 1L) "1 change waits to be sent" else "$waiting changes wait to be sent",
                    color = Palette.faint, style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        TextButton({ model.core.tryNow() }) { Text("Try now", color = Palette.muted) }
    }
}
