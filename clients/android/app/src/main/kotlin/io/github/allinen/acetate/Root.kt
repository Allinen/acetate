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
                    is Page.Playlist -> PlaylistScreen(model, page.id)
                    Page.Liked -> LikedScreen(model)
                }
            }
            // The keyboard takes the place of the bars while typing.
            if (!WindowInsets.isImeVisible) {
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
        SnackbarHost(snacks, Modifier.align(Alignment.BottomCenter).windowInsetsPadding(WindowInsets.navigationBars).padding(bottom = 140.dp))
        AnimatedVisibility(model.nowPlaying, enter = slideInVertically { it }, exit = slideOutVertically { it }) {
            NowPlaying(model)
        }
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
