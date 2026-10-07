package io.github.allinen.acetate

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import uniffi.acetate_ffi.AlbumOrder
import uniffi.acetate_ffi.AlbumView

@Composable
fun LibraryScreen(model: Model) {
    var albums by remember { mutableStateOf<AlbumView?>(null) }
    // A new catalog (or the first) asks for the view again.
    LaunchedEffect(model.catalogVersion) {
        albums = model.core.albums("", AlbumOrder.TITLE)
    }
    Column(Modifier.fillMaxSize().safeDrawingPadding()) {
        val s = model.sync
        Text(
            when {
                s.error != null && !s.haveCatalog -> s.error!!
                !s.haveCatalog -> "Loading the library…"
                else -> "${s.albums} albums · ${s.tracks} tracks" + if (s.refreshing) " · updating…" else ""
            },
            Modifier.padding(16.dp, 8.dp), style = MaterialTheme.typography.labelLarge,
        )
        albums?.let { AlbumList(model, it) }
    }
}
