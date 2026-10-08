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
import androidx.compose.runtime.Composable

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
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.navigationBars)) {
            Box(Modifier.weight(1f)) {
                when (val page = model.pages.lastOrNull()) {
                    null -> LibraryScreen(model)
                    is Page.Album -> AlbumScreen(model, page.id)
                    is Page.Artist -> ArtistScreen(model, page.id)
                }
            }
            if (model.player.current != null) MiniPlayer(model)
        }
        AnimatedVisibility(model.nowPlaying, enter = slideInVertically { it }, exit = slideOutVertically { it }) {
            NowPlaying(model)
        }
    }
}
