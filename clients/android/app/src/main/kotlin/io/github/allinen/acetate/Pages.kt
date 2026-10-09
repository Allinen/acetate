package io.github.allinen.acetate

import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
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
import androidx.compose.material3.TextButton
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
fun PageScaffold(model: Model, title: String, content: @Composable (Modifier) -> Unit) {
    Scaffold(
        contentWindowInsets = WindowInsets(0),
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

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun AlbumScreen(model: Model, id: String) {
    // Read again when a new catalog arrives.
    val album = remember(id, model.catalogVersion) { model.core.album(id) }
    val tracks = remember(id, model.catalogVersion) { model.core.albumTracks(id) }
    PageScaffold(model, album?.title ?: "Album") { modifier ->
        LazyColumn(modifier) {
            if (album != null) item {
                Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Cover(model, album.cover, 240)
                    Spacer(Modifier.height(12.dp))
                    Text(album.title, style = MaterialTheme.typography.headlineSmall, textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                    ArtistLine(model, MarkOn.ALBUM, id, album.artist)
                    Text(
                        listOfNotNull(album.year?.toString(), album.kind).joinToString(" · "),
                        color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                    )
                    Text(
                        "${album.trackCount} tracks · ${duration(album.durationMs)}",
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                        RatingBar(model, MarkOn.ALBUM, id)
                        StarToggle(model, MarkOn.ALBUM, id)
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally), modifier = Modifier.fillMaxWidth()) {
                        Button({
                            if (model.player.shuffle) model.playback.toggleShuffle()
                            model.playback.playAlbum(id, 0uL)
                        }) { Text("Play") }
                        OutlinedButton({
                            if (!model.player.shuffle) model.playback.toggleShuffle()
                            model.playback.playAlbum(id, randomStart(tracks.size))
                        }) { Text("Shuffle") }
                        OutlinedButton({ model.playback.enqueueAlbum(id, false) }) { Text("Queue") }
                    }
                    var sharing by remember { mutableStateOf(false) }
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
                        DownloadButton(model, uniffi.acetate_ffi.PinKind.ALBUM, id)
                        TextButton({ sharing = true }) { Text("Share…") }
                        ExtrasButton(model, MarkOn.ALBUM, id, album.title)
                    }
                    if (sharing) ShareDialog(model, MarkOn.ALBUM, id, album.title) { sharing = false }
                    if (model.editMode) androidx.compose.foundation.layout.FlowRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally)) {
                        OutlinedButton({ model.form = FormRef.Album(id) }) { Text("Edit album") }
                        OutlinedButton({ model.form = FormRef.Covers(id) }) { Text("Covers") }
                        OutlinedButton({ model.edit(tracks.map { it.id }) }, enabled = tracks.isNotEmpty()) { Text("Edit all tracks") }
                        OutlinedButton({ openAlbumOrder(model, id) }, enabled = tracks.size > 1) { Text("Reorder") }
                    }
                }
            }
            items(tracks.size, key = { it }) { i ->
                val t = tracks[i]
                var menu by remember { mutableStateOf(false) }
                val starred = rememberMark(model, t.id).starred
                Box {
                TrackMenu(model, t.id, starred, menu, { menu = false })
                Row(Modifier.fillMaxWidth().combinedClickable(onClick = { model.playback.playAlbum(id, i.toULong()) }, onLongClick = { menu = true }).padding(16.dp, 8.dp)) {
                    Text("${i + 1}", Modifier.width(32.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Column(Modifier.weight(1f)) {
                        Text(t.title, maxLines = 1, color = if (model.player.current?.id == t.id) MaterialTheme.colorScheme.primary else Color.Unspecified)
                        Text(
                            t.artist, maxLines = 1,
                            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    if (starred) Text("★", color = MaterialTheme.colorScheme.primary)
                    if (rememberDownloaded(model, t.id)) Text("↓", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    t.durationMs?.let {
                        Text(duration(it), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
                }
            }
            item { NotesSection(model, MarkOn.ALBUM, id) }
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
                Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(artist.name, style = MaterialTheme.typography.headlineSmall, textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                    Text(
                        "${artist.albumCount} albums · ${artist.trackCount} tracks",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                        RatingBar(model, MarkOn.ARTIST, id)
                        StarToggle(model, MarkOn.ARTIST, id)
                    }
                    // Every track of the artist's albums, in album order.
                    val ids = { albums.flatMap { a -> model.core.albumTracks(a.id).map { it.id } } }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally), modifier = Modifier.fillMaxWidth()) {
                        Button({
                            if (model.player.shuffle) model.playback.toggleShuffle()
                            model.playback.playTracks(ids(), 0uL)
                        }) { Text("Play") }
                        OutlinedButton({
                            if (!model.player.shuffle) model.playback.toggleShuffle()
                            ids().let { model.playback.playTracks(it, randomStart(it.size)) }
                        }) { Text("Shuffle") }
                    }
                    var sharing by remember { mutableStateOf(false) }
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
                        TextButton({ sharing = true }) { Text("Share…") }
                        ExtrasButton(model, MarkOn.ARTIST, id, artist.name)
                    }
                    if (sharing) ShareDialog(model, MarkOn.ARTIST, id, artist.name) { sharing = false }
                    if (model.editMode) OutlinedButton({ model.form = FormRef.Artist(id) }) { Text("Edit artist") }
                }
            }
            items(albums.size, key = { it }) { i -> AlbumItem(model, albums[i]) }
            item { NotesSection(model, MarkOn.ARTIST, id) }
        }
    }
}

/** Shuffle starts anywhere in the list, not at its first track. */
fun randomStart(size: Int): ULong = if (size > 0) kotlin.random.Random.nextInt(size).toULong() else 0uL

/** Reorder mode for an album's tracks: saved as the places that change, in one request. */
fun openAlbumOrder(model: Model, album: String) {
    val rows = model.core.albumOrder(album)
    model.reorder = ReorderSpec(
        "Reorder tracks",
        rows.mapIndexed { i, r -> ReorderItem(r.id, "${r.number ?: (i + 1)}  ${r.title}", r.artist) },
    ) { ids ->
        try {
            model.core.saveOrder(album, ids)
            model.core.startSync()
            null
        } catch (e: uniffi.acetate_ffi.CoreException.Failed) {
            e.reason
        }
    }
}
