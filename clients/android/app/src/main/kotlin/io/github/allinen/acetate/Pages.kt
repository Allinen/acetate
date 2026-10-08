package io.github.allinen.acetate

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import uniffi.acetate_ffi.MarkOn
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PageScaffold(model: Model, title: String, content: @Composable (Modifier) -> Unit) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(title, maxLines = 1) },
                navigationIcon = {
                    IconButton({ model.pages.removeAt(model.pages.lastIndex) }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
            )
        },
    ) { padding -> content(Modifier.padding(padding).fillMaxSize()) }
}

@Composable
fun AlbumScreen(model: Model, id: String) {
    // Read again when a new catalog arrives.
    val album = remember(id, model.catalogVersion) { model.core.album(id) }
    val tracks = remember(id, model.catalogVersion) { model.core.albumTracks(id) }
    PageScaffold(model, album?.title ?: "Album") { modifier ->
        LazyColumn(modifier) {
            if (album != null) item {
                Column(Modifier.fillMaxWidth().padding(16.dp)) {
                    Cover(model, album.cover, 240)
                    Spacer(Modifier.height(12.dp))
                    Text(album.title, style = MaterialTheme.typography.headlineSmall)
                    Text(
                        listOfNotNull(album.artist, album.year?.toString(), album.kind).joinToString(" · "),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Text(
                        "${album.trackCount} tracks · ${duration(album.durationMs)}",
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        RatingBar(model, MarkOn.ALBUM, id)
                        Spacer(Modifier.weight(1f))
                        StarToggle(model, MarkOn.ALBUM, id)
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button({
                            if (model.player.shuffle) model.playback.toggleShuffle()
                            model.playback.playAlbum(id, 0uL)
                        }) { Text("Play") }
                        OutlinedButton({
                            if (!model.player.shuffle) model.playback.toggleShuffle()
                            model.playback.playAlbum(id, 0uL)
                        }) { Text("Shuffle") }
                        OutlinedButton({ model.playback.enqueueAlbum(id, false) }) { Text("Queue") }
                    }
                }
            }
            items(tracks.size, key = { it }) { i ->
                val t = tracks[i]
                Row(Modifier.fillMaxWidth().clickable { model.playback.playAlbum(id, i.toULong()) }.padding(16.dp, 8.dp)) {
                    Text("${i + 1}", Modifier.width(32.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Column(Modifier.weight(1f)) {
                        Text(t.title, maxLines = 1, color = if (model.player.current?.id == t.id) MaterialTheme.colorScheme.primary else Color.Unspecified)
                        Text(
                            t.artist, maxLines = 1,
                            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    if (rememberMark(model, t.id).starred) Text("★", color = MaterialTheme.colorScheme.primary)
                    t.durationMs?.let {
                        Text(duration(it), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
    }
}

@Composable
fun ArtistScreen(model: Model, id: String) {
    val artist = remember(id, model.catalogVersion) { model.core.artist(id) }
    val albums = remember(id, model.catalogVersion) { model.core.artistAlbums(id) }
    PageScaffold(model, artist?.name ?: "Artist") { modifier ->
        LazyColumn(modifier) {
            if (artist != null) item {
                Column(Modifier.padding(16.dp)) {
                    Text(artist.name, style = MaterialTheme.typography.headlineSmall)
                    Text(
                        "${artist.albumCount} albums · ${artist.trackCount} tracks",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        RatingBar(model, MarkOn.ARTIST, id)
                        Spacer(Modifier.weight(1f))
                        StarToggle(model, MarkOn.ARTIST, id)
                    }
                }
            }
            items(albums.size, key = { it }) { i -> AlbumItem(model, albums[i]) }
        }
    }
}
