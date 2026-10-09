package io.github.allinen.acetate

import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import uniffi.acetate_ffi.Chip
import uniffi.acetate_ffi.SearchResult

/** Chips that complete the line being typed (keys, then values from the library). */
@Composable
fun Suggestions(model: Model, text: String, pick: (String) -> Unit) {
    var chips by remember { mutableStateOf<List<Chip>>(emptyList()) }
    LaunchedEffect(text, model.catalogVersion) { chips = model.core.suggest(text) }
    if (chips.isEmpty()) return
    LazyRow(
        Modifier.fillMaxWidth(), contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp, 4.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        items(chips.size, key = { chips[it].line }) { i ->
            val chip = chips[i]
            Text(
                chip.label, maxLines = 1, style = MaterialTheme.typography.labelLarge, color = Palette.accent,
                modifier = Modifier.clip(RoundedCornerShape(50))
                    .border(1.dp, Palette.line, RoundedCornerShape(50))
                    .clickable { pick(chip.line) }
                    .padding(12.dp, 7.dp),
            )
        }
    }
}

/** The Search tab: results of every kind above, the line to type at the bottom (above the
 *  keyboard) with its suggestions over it. */
@Composable
fun SearchScreen(model: Model) {
    val text = model.searchText
    var found by remember { mutableStateOf(SearchResult(emptyList(), emptyList(), emptyList(), emptyList())) }
    LaunchedEffect(text, model.catalogVersion, model.playlistsVersion) { found = model.core.search(text, 20uL) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { if (model.searchText.isEmpty()) focus.requestFocus() }
    Column(Modifier.windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal))) {
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), reverseLayout = false) {
            if (found.artists.isNotEmpty()) {
                item { Header("Artists") }
                items(found.artists.size, key = { "a" + found.artists[it].id }) { ArtistItem(model, found.artists[it]) }
            }
            if (found.albums.isNotEmpty()) {
                item { Header("Albums") }
                items(found.albums.size, key = { "b" + found.albums[it].id }) { AlbumItem(model, found.albums[it]) }
            }
            if (found.playlists.isNotEmpty()) {
                item { Header("Playlists") }
                items(found.playlists.size, key = { "p" + found.playlists[it].id }) {
                    val p = found.playlists[it]
                    ListLine(p.name, "${p.entries} tracks", Modifier.clickable { model.pages.add(Page.Playlist(p.id)) })
                }
            }
            if (found.tracks.isNotEmpty()) {
                item { Header("Tracks") }
                val ids = found.tracks.map { it.id }
                items(found.tracks.size, key = { "t" + found.tracks[it].id }) { i ->
                    TrackItem(model, found.tracks[i]) { model.playback.playTracks(ids, i.toULong()) }
                }
            }
        }
        Suggestions(model, text) { model.searchText = it }
        LineField(
            text, { model.searchText = it }, "Search, or artist: year: …",
            Modifier.fillMaxWidth().padding(16.dp, 4.dp, 16.dp, 8.dp).focusRequester(focus),
        )
    }
}

@Composable
private fun Header(title: String) {
    Text(
        title, Modifier.padding(16.dp, 14.dp, 16.dp, 4.dp),
        style = MaterialTheme.typography.labelLarge, color = Palette.accent,
    )
}

/** A line of text for the query language. When the text is changed from outside (a chip
 *  chosen), the cursor goes to its end. */
@Composable
fun LineField(text: String, onChange: (String) -> Unit, hint: String, modifier: Modifier, onDone: (() -> Unit)? = null) {
    var value by remember { mutableStateOf(TextFieldValue(text, TextRange(text.length))) }
    if (value.text != text) value = TextFieldValue(text, TextRange(text.length))
    OutlinedTextField(
        value, { value = it; onChange(it.text) }, modifier,
        placeholder = { Text(hint) }, singleLine = true,
        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(imeAction = if (onDone != null) androidx.compose.ui.text.input.ImeAction.Done else androidx.compose.ui.text.input.ImeAction.Default),
        keyboardActions = androidx.compose.foundation.text.KeyboardActions(onDone = { onDone?.invoke() }),
    )
}
