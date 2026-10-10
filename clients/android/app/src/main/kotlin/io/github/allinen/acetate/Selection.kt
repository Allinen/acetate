package io.github.allinen.acetate

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import uniffi.acetate_ffi.MarkOn
import uniffi.acetate_ffi.PinKind

/** What can be done to the tracks picked in a list (long press → Select, then tap more). */
@Composable
fun SelectionBar(model: Model) {
    val ids = model.selected.toList()
    var picking by remember { mutableStateOf(false) }
    Row(
        Modifier.fillMaxWidth().background(Palette.sidebar).horizontalScroll(rememberScrollState()).padding(8.dp, 0.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("${ids.size} selected", Modifier.padding(horizontal = 8.dp), style = MaterialTheme.typography.labelLarge)
        TextButton({ model.playback.enqueueTracks(ids, true); model.selected.clear() }) { Text("Play next") }
        TextButton({ model.playback.enqueueTracks(ids, false); model.selected.clear() }) { Text("Queue") }
        TextButton({ picking = true }) { Text("Playlist…") }
        // Star all, or (when they all are) unstar all; the same for downloads.
        TextButton({
            val all = ids.all { model.core.mark(it).starred }
            ids.forEach { model.core.setStarred(MarkOn.TRACK, it, !all) }
            model.selected.clear()
        }) { Text("Star") }
        TextButton({
            val all = ids.all { model.core.isDownloading(PinKind.TRACK, it) }
            ids.forEach { model.core.download(PinKind.TRACK, it, !all) }
            model.selected.clear()
        }) { Text("Download") }
        if (model.editMode) TextButton({ model.edit(ids); model.selected.clear() }) { Text("Edit…") }
        TextButton({ model.selected.clear() }) { Text("Clear") }
    }
    if (picking) PlaylistPicker(model, ids) { picking = false; model.selected.clear() }
}
