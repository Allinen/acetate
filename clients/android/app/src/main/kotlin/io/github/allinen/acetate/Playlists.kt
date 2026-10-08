package io.github.allinen.acetate

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import uniffi.acetate_ffi.EntryRow
import uniffi.acetate_ffi.TrackRow

/** The Playlists tab: Liked songs (the starred tracks), then the playlists. */
@Composable
fun PlaylistList(model: Model) {
    val playlists = remember(model.playlistsVersion, model.catalogVersion) { model.core.playlists() }
    val liked = remember(model.marksVersion, model.catalogVersion) { model.core.liked().size }
    var creating by remember { mutableStateOf(false) }
    LazyColumn(Modifier.fillMaxWidth()) {
        item {
            Row(Modifier.fillMaxWidth().padding(16.dp, 4.dp)) {
                OutlinedButton({ creating = true }) { Text("New playlist") }
            }
        }
        item {
            ListLine("Liked songs", "$liked tracks", Modifier.clickable { model.pages.add(Page.Liked) })
        }
        items(playlists.size, key = { playlists[it].id }) { i ->
            val p = playlists[i]
            val facts = listOfNotNull(
                if (p.mine) null else p.owner, "${p.entries} tracks", if (p.entries > 0) duration(p.durationMs) else null,
            ).joinToString(" · ")
            ListLine(p.name, facts, Modifier.clickable { model.pages.add(Page.Playlist(p.id)) })
        }
    }
    if (creating) NamePlaylist(model, emptyList()) { creating = false }
}

@Composable
fun ListLine(title: String, subtitle: String, modifier: Modifier) {
    Column(modifier.fillMaxWidth().padding(16.dp, 10.dp)) {
        Text(title, style = MaterialTheme.typography.bodyLarge, maxLines = 1)
        Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** Asks for a name and makes a playlist with `tracks` in it. */
@Composable
private fun NamePlaylist(model: Model, tracks: List<String>, close: () -> Unit) {
    var name by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = close,
        title = { Text("New playlist") },
        text = { OutlinedTextField(name, { name = it }, singleLine = true, placeholder = { Text("Name") }) },
        confirmButton = {
            TextButton({
                model.core.createPlaylist(name.trim(), tracks)
                close()
            }, enabled = name.isNotBlank()) { Text("Make") }
        },
        dismissButton = { TextButton(close) { Text("Cancel") } },
    )
}

/** Where to add these tracks: one of the user's playlists, or a new one. */
@Composable
fun PlaylistPicker(model: Model, tracks: List<String>, close: () -> Unit) {
    val mine = remember(model.playlistsVersion) { model.core.playlists().filter { it.mine } }
    var creating by remember { mutableStateOf(false) }
    if (creating) {
        NamePlaylist(model, tracks, close)
        return
    }
    AlertDialog(
        onDismissRequest = close,
        title = { Text("Add to playlist") },
        text = {
            LazyColumn {
                item { TextButton({ creating = true }) { Text("New playlist…") } }
                items(mine.size, key = { mine[it].id }) { i ->
                    val p = mine[i]
                    Text(
                        p.name, maxLines = 1,
                        modifier = Modifier.fillMaxWidth().clickable {
                            model.core.addToPlaylist(p.id, p.name, tracks)
                            close()
                        }.padding(8.dp, 12.dp),
                    )
                }
            }
        },
        confirmButton = { TextButton(close) { Text("Cancel") } },
    )
}

@Composable
fun LikedScreen(model: Model) {
    val rows = remember(model.marksVersion, model.catalogVersion) { model.core.liked() }
    TracksPage(model, "Liked songs", "${rows.size} tracks", rows.map { it to null }, mine = false, playlist = null)
}

@Composable
fun PlaylistScreen(model: Model, id: String) {
    val page = remember(id, model.playlistsVersion, model.catalogVersion) { model.core.playlist(id) }
    val facts = when {
        page == null -> ""
        page.entries.isEmpty() && page.loading -> "Loading…"
        else -> listOfNotNull(if (page.mine) null else page.owner, "${page.entries.size} tracks", page.error).joinToString(" · ")
    }
    TracksPage(
        model, page?.name?.ifEmpty { "Playlist" } ?: "Playlist", facts,
        page?.entries?.map { it.track to it.entry } ?: emptyList(), mine = page?.mine == true, playlist = page,
    )
}

/** A page of tracks (Liked songs or a playlist): play, shuffle, and the rows. */
@Composable
private fun TracksPage(
    model: Model, title: String, facts: String, rows: List<Pair<TrackRow, String?>>, mine: Boolean,
    playlist: uniffi.acetate_ffi.PlaylistPage?,
) {
    val ids = rows.map { it.first.id }
    PageScaffold(model, title) { modifier ->
        LazyColumn(modifier) {
            item {
                Column(Modifier.fillMaxWidth().padding(16.dp)) {
                    Text(title, style = MaterialTheme.typography.headlineSmall)
                    if (playlist?.description?.isNotEmpty() == true) Text(playlist.description, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text(facts, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Spacer(Modifier.height(8.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button({
                            if (model.player.shuffle) model.playback.toggleShuffle()
                            model.playback.playTracks(ids, 0uL)
                        }, enabled = ids.isNotEmpty()) { Text("Play") }
                        OutlinedButton({
                            if (!model.player.shuffle) model.playback.toggleShuffle()
                            model.playback.playTracks(ids, randomStart(ids.size))
                        }, enabled = ids.isNotEmpty()) { Text("Shuffle") }
                    }
                }
            }
            items(rows.size, key = { rows[it].second ?: rows[it].first.id }) { i ->
                val (row, entry) = rows[i]
                TrackItem(model, row, extra = { close ->
                    if (mine && entry != null && playlist != null) {
                        if (i > 0) DropdownMenuItem({ Text("Move up") }, { model.core.moveInPlaylist(playlist.id, entry, -1); close() })
                        if (i < rows.size - 1) DropdownMenuItem({ Text("Move down") }, { model.core.moveInPlaylist(playlist.id, entry, 1); close() })
                        DropdownMenuItem({ Text("Remove from playlist") }, { model.core.removeFromPlaylist(playlist.id, listOf(entry)); close() })
                    }
                }) { model.playback.playTracks(ids, i.toULong()) }
            }
        }
    }
}
