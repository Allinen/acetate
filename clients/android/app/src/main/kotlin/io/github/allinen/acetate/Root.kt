package io.github.allinen.acetate

import androidx.activity.compose.BackHandler
import androidx.compose.runtime.Composable

@Composable
fun Root(model: Model) {
    if (model.session == null) {
        LoginScreen(model)
        return
    }
    BackHandler(enabled = model.pages.isNotEmpty()) { model.pages.removeAt(model.pages.lastIndex) }
    when (val page = model.pages.lastOrNull()) {
        null -> LibraryScreen(model)
        is Page.Album -> AlbumScreen(model, page.id)
        is Page.Artist -> ArtistScreen(model, page.id)
    }
}
