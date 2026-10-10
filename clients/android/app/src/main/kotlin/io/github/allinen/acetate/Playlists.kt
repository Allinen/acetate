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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import uniffi.acetate_ffi.EntryRow
import uniffi.acetate_ffi.MarkOn
import uniffi.acetate_ffi.PinKind
import uniffi.acetate_ffi.TrackRow

/** The Playlists tab: Liked songs (the starred tracks), then the playlists. */
@Composable
fun PlaylistList(model: Model, filter: String) {
    val words = filter.lowercase().split(' ').filter { it.isNotEmpty() }
    val playlists = remember(model.playlistsVersion, model.catalogVersion, filter) {
        model.core.playlists().filter { p -> words.all { it in p.name.lowercase() } }
    }
    val liked = remember(model.marksVersion, model.catalogVersion) { model.core.liked().size }
    var creating by remember { mutableStateOf(false) }
    LazyColumn(Modifier.fillMaxWidth()) {
        item {
            Row(Modifier.fillMaxWidth().padding(16.dp, 4.dp)) {
                OutlinedButton({ creating = true }) { Text("New playlist") }
            }
        }
        if (words.all { it in "liked songs" }) item {
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
        text = { FocusedField(name, { name = it }, label = { Text("Name") }) },
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

/** A row of a tracks page: `entry` is its place in a playlist; `faded`: taken out (unstarred) since the page opened. */
data class ListRow(val track: TrackRow, val entry: String?, val faded: Boolean)

/** A playlist entry taken out since the page opened, with its place in the list then. */
private class Ghost(val at: Int, val row: EntryRow)

@Composable
fun LikedScreen(model: Model) {
    // The liked songs when the page opened, in their order: one unstarred meanwhile stays
    // there, faded, until the page is opened again; starring it again puts it back in place.
    val seen = remember { model.core.liked() }
    val rows = remember(model.marksVersion, model.catalogVersion) {
        val known = seen.map { it.id }.toSet()
        val fresh = model.core.liked().filter { it.id !in known }
        (fresh + seen).map { ListRow(it, null, !model.core.mark(it.id).starred) }
    }
    TracksPage(model, "Liked songs", "${rows.count { !it.faded }} tracks", rows, mine = false, playlist = null) { row ->
        model.core.setStarred(MarkOn.TRACK, row.track.id, true)
    }
}

@Composable
fun PlaylistScreen(model: Model, id: String) {
    val removed = remember(id) { mutableStateListOf<Ghost>() }
    val page = remember(id, model.playlistsVersion, model.catalogVersion) { model.core.playlist(id) }
    // Entries taken out meanwhile stay in their place, faded (unless they are back).
    val rows = remember(page, removed.toList()) {
        val live = page?.entries ?: emptyList()
        val list = live.map { ListRow(it.track, it.entry, false) }.toMutableList()
        removed.filter { g -> live.none { it.entry == g.row.entry } }.sortedBy { it.at }.forEach { g -> list.add(minOf(g.at, list.size), ListRow(g.row.track, g.row.entry, true)) }
        list
    }
    val live = rows.count { !it.faded }
    val facts = when {
        page == null -> ""
        page.entries.isEmpty() && page.loading -> "Loading…"
        else -> listOfNotNull(if (page.mine) null else page.owner, "$live tracks", page.error).joinToString(" · ")
    }
    TracksPage(
        model, page?.name?.ifEmpty { "Playlist" } ?: "Playlist", facts, rows, mine = page?.mine == true, playlist = page,
        onRemove = { row ->
            val at = rows.indexOfFirst { it.entry == row.entry }
            if (page != null && at >= 0 && row.entry != null) {
                removed.add(Ghost(at, EntryRow(row.entry, row.track)))
                model.core.removeFromPlaylist(page.id, listOf(row.entry))
            }
        },
    ) { row ->
        // Back where it was: before the playlist's entry that now stands at its place.
        val n = rows.indexOfFirst { it.entry == row.entry }
        if (page != null && n >= 0) {
            removed.removeAll { it.row.entry == row.entry }
            model.core.addToPlaylistAt(page.id, page.name, listOf(row.track.id), rows.take(n).count { !it.faded }.toULong())
        }
    }
}

/** A page of tracks (Liked songs or a playlist): play, shuffle, and the rows. */
@Composable
private fun TracksPage(
    model: Model, title: String, facts: String, rows: List<ListRow>, mine: Boolean,
    playlist: uniffi.acetate_ffi.PlaylistPage?, onRemove: (ListRow) -> Unit = {}, onRestore: (ListRow) -> Unit,
) {
    // What the filter leaves (its places in `rows`, in order): playing starts from, and goes
    // through, only those. (What was taken out is never played.)
    var filter by remember { mutableStateOf("") }
    val all = remember(rows) { rows.map { it.track.id } }
    var visible by remember { mutableStateOf<List<Int>?>(null) }
    LaunchedEffect(filter, all, model.catalogVersion) {
        visible = if (filter.isBlank()) null else model.core.filterList(all, filter).map { it.toInt() }
    }
    val shown = visible?.map { it to rows[it] } ?: rows.mapIndexed { i, r -> i to r }
    val ids = shown.filter { !it.second.faded }.map { it.second.track.id }
    val anyFaded = rows.any { it.faded }
    PageScaffold(model, title) { modifier ->
        LazyColumn(modifier) {
            item {
                Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = androidx.compose.ui.Alignment.CenterHorizontally) {
                    val center = androidx.compose.ui.text.style.TextAlign.Center
                    Text(title, style = MaterialTheme.typography.headlineSmall, textAlign = center)
                    if (playlist?.description?.isNotEmpty() == true) Text(playlist.description, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = center)
                    Text(facts, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = center)
                    Spacer(Modifier.height(8.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp, androidx.compose.ui.Alignment.CenterHorizontally), modifier = Modifier.fillMaxWidth()) {
                        Button({
                            if (model.player.shuffle) model.playback.toggleShuffle()
                            model.playback.playTracks(ids, 0uL)
                        }, enabled = ids.isNotEmpty()) { Text("Play") }
                        OutlinedButton({
                            if (!model.player.shuffle) model.playback.toggleShuffle()
                            model.playback.playTracks(ids, randomStart(ids.size))
                        }, enabled = ids.isNotEmpty()) { Text("Shuffle") }
                    }
                    if (playlist != null) DownloadButton(model, PinKind.PLAYLIST, playlist.id) else if (title == "Liked songs") DownloadButton(model, PinKind.LIKED, "")
                    if (model.editMode && mine && playlist != null) PlaylistEdits(model, playlist, rows.filter { !it.faded })
                }
            }
            if (rows.size > 5 || filter.isNotEmpty()) item {
                Column {
                    LineField(filter, { filter = it }, "Filter", Modifier.fillMaxWidth().padding(16.dp, 4.dp))
                    Suggestions(model, filter) { filter = it }
                }
            }
            items(shown.size, key = { shown[it].second.entry ?: shown[it].second.track.id }) { n ->
                val (i, row) = shown[n]
                TrackItem(model, row.track, faded = row.faded, extra = { close ->
                    if (row.faded) {
                        if (playlist == null || mine) DropdownMenuItem({ Text("Put back") }, { onRestore(row); close() })
                    } else if (mine && row.entry != null && playlist != null) {
                        // Moving makes sense in the whole playlist, not in what a filter leaves,
                        // nor while the tracks just taken out are shown.
                        val moves = filter.isBlank() && !anyFaded
                        if (moves && i > 0) DropdownMenuItem({ Text("Move up") }, { model.core.moveInPlaylist(playlist.id, row.entry, -1); close() })
                        if (moves && i < rows.size - 1) DropdownMenuItem({ Text("Move down") }, { model.core.moveInPlaylist(playlist.id, row.entry, 1); close() })
                        DropdownMenuItem({ Text("Remove from playlist") }, { onRemove(row); close() })
                    }
                }) {
                    if (row.faded) model.notice = "Taken out. Long press it to put it back."
                    else model.playback.playTracks(ids, shown.take(n).count { !it.second.faded }.toULong())
                }
            }
        }
    }
}

/** Edit mode on your own playlist: rename, put the entries in another order, delete (to the trash). */
@Composable
private fun PlaylistEdits(model: Model, playlist: uniffi.acetate_ffi.PlaylistPage, rows: List<ListRow>) {
    var renaming by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, androidx.compose.ui.Alignment.CenterHorizontally)) {
        OutlinedButton({ renaming = true }) { Text("Rename") }
        OutlinedButton({
            model.reorder = ReorderSpec("Reorder ${playlist.name}", rows.mapNotNull { r -> r.entry?.let { ReorderItem(it, r.track.title, r.track.artist) } }) { ids ->
                model.core.reorderPlaylist(playlist.id, ids)
                null
            }
        }, enabled = rows.size > 1) { Text("Reorder") }
        OutlinedButton({ deleting = true }) { Text("Delete") }
    }
    if (renaming) {
        var name by remember { mutableStateOf(playlist.name) }
        AlertDialog(
            onDismissRequest = { renaming = false },
            title = { Text("Rename playlist") },
            text = { FocusedField(name, { name = it }) },
            confirmButton = { TextButton({ model.core.renamePlaylist(playlist.id, name.trim()); renaming = false }, enabled = name.isNotBlank() && name.trim() != playlist.name) { Text("Rename") } },
            dismissButton = { TextButton({ renaming = false }) { Text("Cancel") } },
        )
    }
    if (deleting) AlertDialog(
        onDismissRequest = { deleting = false },
        title = { Text("Delete “${playlist.name}”?") },
        text = { Text("It goes to the trash; the server keeps it there.") },
        confirmButton = { TextButton({ deleting = false; model.core.deletePlaylist(playlist.id, playlist.name); model.pages.removeAt(model.pages.lastIndex) }) { Text("Delete") } },
        dismissButton = { TextButton({ deleting = false }) { Text("Cancel") } },
    )
}
