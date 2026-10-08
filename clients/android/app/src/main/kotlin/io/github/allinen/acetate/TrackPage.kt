package io.github.allinen.acetate

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import uniffi.acetate_ffi.MarkOn
import uniffi.acetate_ffi.PinKind

/** Opens an artist's page (from Now playing too: that closes). */
fun openArtist(model: Model, id: String) {
    model.nowPlaying = false
    model.pages.add(Page.Artist(id))
}

fun openAlbum(model: Model, id: String) {
    model.nowPlaying = false
    model.pages.add(Page.Album(id))
}

fun openTrack(model: Model, id: String) {
    model.nowPlaying = false
    model.pages.add(Page.Track(id))
}

/**
 * The artists credited on a track or album, each one to tap (to their page). Producers and
 * the like are left out of the line (they're on the track's page); with none to show,
 * `fallback` is the line as text.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ArtistLine(model: Model, on: MarkOn, id: String, fallback: String, center: Boolean = true) {
    val links = remember(on, id, model.catalogVersion) {
        model.core.artistsOf(on, id).filter { it.role == "primary" || it.role == "featured" }
    }
    val style = MaterialTheme.typography.bodyLarge
    if (links.isEmpty()) {
        Text(fallback, style = style, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = if (center) TextAlign.Center else null)
        return
    }
    FlowRow(
        Modifier.fillMaxWidth(),
        horizontalArrangement = if (center) Arrangement.Center else Arrangement.Start,
    ) {
        links.forEachIndexed { i, a ->
            val featured = a.role == "featured" && links.take(i).none { it.role == "featured" }
            if (i > 0) Text(if (featured) " feat. " else ", ", style = style, color = MaterialTheme.colorScheme.onSurfaceVariant)
            else if (a.role == "featured") Text("feat. ", style = style, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(a.name, style = style, color = Palette.accent, modifier = Modifier.clickable { openArtist(model, a.id) })
        }
    }
}

/** A track's own page: who and where, what to do with it, then more below (lyrics now;
 *  what else we know of it goes further down as it comes). */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun TrackScreen(model: Model, id: String) {
    val page = remember(id, model.catalogVersion) { model.core.trackPage(id) }
    val lyrics = remember(id, model.detailsVersion) { model.core.lyrics(id) }
    val mark = rememberMark(model, id)
    PageScaffold(model, page?.row?.title ?: "Track") { modifier ->
        LazyColumn(modifier) {
            if (page != null) {
                item {
                    Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                        Cover(model, page.row.cover, 240)
                        Spacer(Modifier.height(12.dp))
                        Text(page.row.title, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
                        ArtistLine(model, MarkOn.TRACK, id, page.row.artist)
                        if (page.albums.isNotEmpty()) {
                            FlowRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
                                Text("on ", color = MaterialTheme.colorScheme.onSurfaceVariant)
                                page.albums.forEachIndexed { i, a ->
                                    if (i > 0) Text(", ", color = MaterialTheme.colorScheme.onSurfaceVariant)
                                    Text(a.title, color = Palette.accent, modifier = Modifier.clickable { openAlbum(model, a.id) })
                                }
                            }
                        }
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                            RatingBar(model, MarkOn.TRACK, id)
                            StarToggle(model, MarkOn.TRACK, id)
                        }
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally)) {
                            Button({ model.playback.playTracks(listOf(id), 0uL) }) { Text("Play") }
                            OutlinedButton({ model.playback.enqueueTracks(listOf(id), true) }) { Text("Play next") }
                            OutlinedButton({ model.playback.enqueueTracks(listOf(id), false) }) { Text("Queue") }
                        }
                        var sharing by remember { mutableStateOf(false) }
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
                            DownloadButton(model, PinKind.TRACK, id)
                            TextButton({ sharing = true }) { Text("Share…") }
                            ExtrasButton(model, MarkOn.TRACK, id, page.row.title)
                        }
                        if (sharing) ShareDialog(model, MarkOn.TRACK, id, page.row.title) { sharing = false }
                    }
                }
                item {
                    Column(Modifier.fillMaxWidth().padding(16.dp, 0.dp)) {
                        SectionHeader("Details")
                        val facts = listOfNotNull(
                            page.releaseDate?.let { "Released" to it },
                            page.row.durationMs?.let { "Length" to duration(it) },
                            if (page.genres.isNotEmpty()) "Genres" to page.genres.joinToString(", ") else null,
                            page.codec?.let { c ->
                                "File" to listOfNotNull(c, page.bitrate?.let { "$it kbps" }, page.sampleRate?.let { "$it Hz" }).joinToString(" · ")
                            },
                            "Status" to page.status,
                            if (mark.playCount > 0) "Plays" to mark.playCount.toString() else null,
                        )
                        facts.forEach { (name, value) ->
                            Row(Modifier.padding(vertical = 2.dp)) {
                                Text(name, Modifier.weight(0.3f), color = MaterialTheme.colorScheme.onSurfaceVariant)
                                Text(value, Modifier.weight(0.7f))
                            }
                        }
                    }
                }
                // More of what we know of the track goes below this (related tracks, its
                // history…), each in a section of its own.
                if (lyrics != null) item {
                    Column(Modifier.fillMaxWidth().padding(16.dp, 8.dp)) {
                        SectionHeader("Lyrics")
                        lyrics.lines.forEach { line ->
                            line.heading?.let { Text(it, Modifier.padding(top = 10.dp), style = MaterialTheme.typography.labelLarge, color = Palette.accent) }
                            Text(line.text.ifEmpty { " " }, Modifier.padding(vertical = 2.dp), style = MaterialTheme.typography.bodyLarge)
                        }
                        Spacer(Modifier.height(24.dp))
                    }
                }
            }
        }
    }
}

@Composable
fun SectionHeader(title: String) {
    Text(title, Modifier.padding(top = 16.dp, bottom = 6.dp), style = MaterialTheme.typography.labelLarge, color = Palette.accent)
}
