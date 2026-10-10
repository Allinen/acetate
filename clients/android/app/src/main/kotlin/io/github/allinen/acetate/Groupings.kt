package io.github.allinen.acetate

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import uniffi.acetate_ffi.CoreException
import uniffi.acetate_ffi.GroupingInfo
import uniffi.acetate_ffi.GroupingMemberRow

/** Every grouping (collectives, eras, projects…): kind chips, a filter by name, a row each. */
@Composable
fun GroupingsScreen(model: Model) {
    var filter by remember { mutableStateOf("") }
    var kind by remember { mutableStateOf<String?>(null) }
    val all = remember(model.catalogVersion) { model.core.groupings("") }
    val kinds = remember(all) { all.groupBy { it.kind }.toList().sortedBy { it.first } }
    val shown = remember(all, filter, kind) { model.core.groupings(filter).filter { kind == null || it.kind == kind } }
    PageScaffold(model, "Collectives & groupings") { modifier ->
        LazyColumn(modifier) {
            item {
                Column {
                    LineField(filter, { filter = it }, "Filter by name", Modifier.fillMaxWidth().padding(16.dp, 4.dp))
                    Row(Modifier.horizontalScroll(rememberScrollState()).padding(16.dp, 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        FilterChip(kind == null, { kind = null }, { Text("All ${all.size}") })
                        kinds.forEach { (k, list) ->
                            FilterChip(kind == k, { kind = if (kind == k) null else k }, { Text("${k.replaceFirstChar { it.uppercase() }} ${list.size}") })
                        }
                    }
                }
            }
            if (shown.isEmpty()) item { Column(Modifier.padding(16.dp)) { Muted("Nothing matches.") } }
            items(shown.size, key = { shown[it].id }) { i ->
                val g = shown[i]
                ListLine(g.name, "${g.kind} · ${g.tracks} tracks · ${g.albums} releases", Modifier.clickable { model.pages.add(Page.Grouping(g.id)) })
            }
        }
    }
}

/** A grouping: what it is, who is in it, its releases and its tracks. */
@Composable
fun GroupingScreen(model: Model, id: String) {
    val grouping = remember(id, model.catalogVersion) { model.core.grouping(id) }
    val albums = remember(id, model.catalogVersion) { model.core.groupingAlbums(id) }
    val tracks = remember(id, model.catalogVersion) { model.core.groupingTracks(id) }
    val extras = remember(id, model.catalogVersion) { model.core.groupingExtras(id) }
    // The members as the catalog has them, then the server's description and counts.
    var info by remember(id) { mutableStateOf<GroupingInfo?>(null) }
    LaunchedEffect(id, model.connection.offline) {
        try { info = model.core.groupingInfo(id) } catch (e: CoreException) { /* the catalog's members stay */ }
    }
    val members: List<GroupingMemberRow> = info?.members ?: remember(id, model.catalogVersion) { model.core.groupingMembers(id) }
    val ids = tracks.map { it.id }
    PageScaffold(model, grouping?.name ?: "Grouping") { modifier ->
        LazyColumn(modifier) {
            if (grouping == null) item { Column(Modifier.padding(16.dp)) { Muted("This grouping isn't in the library (anymore).") } }
            else item {
                Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    val center = TextAlign.Center
                    Text(grouping.kind.uppercase(), style = MaterialTheme.typography.labelSmall, color = Palette.accent)
                    Text(grouping.name, style = MaterialTheme.typography.headlineSmall, textAlign = center)
                    info?.description?.takeIf { it.isNotEmpty() }?.let {
                        Text(it, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = center)
                    }
                    Text(
                        "${tracks.size} tracks · ${albums.size} releases",
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    if (members.isNotEmpty()) {
                        Spacer(Modifier.height(8.dp))
                        Text("Members", style = MaterialTheme.typography.labelMedium, color = Palette.faint)
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterHorizontally)) {
                            members.forEach { m ->
                                Text(
                                    m.name + (m.tracks?.let { " ($it)" } ?: ""),
                                    Modifier.clickable { model.pages.add(Page.Artist(m.artist)) }.padding(vertical = 6.dp),
                                    color = Palette.accent,
                                )
                            }
                        }
                    }
                    Spacer(Modifier.height(8.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally), modifier = Modifier.fillMaxWidth()) {
                        Button({
                            if (model.player.shuffle) model.playback.toggleShuffle()
                            model.playback.playTracks(ids, 0uL)
                        }, enabled = ids.isNotEmpty()) { Text("Play") }
                        OutlinedButton({
                            if (!model.player.shuffle) model.playback.toggleShuffle()
                            model.playback.playTracks(ids, randomStart(ids.size))
                        }, enabled = ids.isNotEmpty()) { Text("Shuffle") }
                    }
                    ExtrasButtonOf(model, extras, grouping.name)
                }
            }
            if (albums.isNotEmpty()) {
                item { Text("Releases ${albums.size}", Modifier.padding(16.dp, 8.dp), style = MaterialTheme.typography.titleMedium) }
                items(albums.size, key = { "a" + albums[it].id }) { i -> AlbumItem(model, albums[i]) }
            }
            if (tracks.isNotEmpty()) {
                item { Text("Tracks ${tracks.size}", Modifier.padding(16.dp, 8.dp), style = MaterialTheme.typography.titleMedium) }
                items(tracks.size, key = { "t" + tracks[it].id }) { i ->
                    TrackItem(model, tracks[i]) { model.playback.playTracks(ids, i.toULong()) }
                }
            }
            if (grouping != null && tracks.isEmpty() && albums.isEmpty()) item { Column(Modifier.padding(16.dp)) { Muted("Nothing in it yet.") } }
        }
    }
}
