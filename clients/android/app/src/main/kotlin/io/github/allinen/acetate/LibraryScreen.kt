package io.github.allinen.acetate

import androidx.compose.material3.Slider
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.PrimaryTabRow
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import uniffi.acetate_ffi.AlbumOrder
import uniffi.acetate_ffi.AlbumView
import uniffi.acetate_ffi.ArtistOrder
import uniffi.acetate_ffi.ArtistView
import uniffi.acetate_ffi.TrackOrder
import uniffi.acetate_ffi.TrackView

private val tabs = listOf("Albums", "Artists", "Tracks")

@Composable
fun LibraryScreen(model: Model) {
    var tab by rememberSaveable { mutableIntStateOf(0) }
    // One filter line per tab, in the library's query language (`artist:… year:…`).
    var filters by rememberSaveable { mutableStateOf(listOf("", "", "")) }
    Column(Modifier.fillMaxSize().safeDrawingPadding()) {
        SyncLine(model)
        PrimaryTabRow(selectedTabIndex = tab) {
            tabs.forEachIndexed { i, name -> Tab(tab == i, { tab = i }, text = { Text(name) }) }
        }
        OutlinedTextField(
            filters[tab], { text -> filters = filters.toMutableList().also { it[tab] = text } },
            Modifier.fillMaxWidth().padding(16.dp, 8.dp),
            placeholder = { Text("Filter") }, singleLine = true,
        )
        val filter = filters[tab]
        when (tab) {
            0 -> {
                var view by remember { mutableStateOf<AlbumView?>(null) }
                // A new catalog (or the first) or a new filter asks for the view again.
                LaunchedEffect(model.catalogVersion, filter) { view = model.core.albums(filter, AlbumOrder.TITLE) }
                view?.let { AlbumList(model, it) }
            }
            1 -> {
                var view by remember { mutableStateOf<ArtistView?>(null) }
                LaunchedEffect(model.catalogVersion, filter) { view = model.core.artists(filter, ArtistOrder.NAME) }
                view?.let { ArtistList(model, it) }
            }
            else -> {
                var view by remember { mutableStateOf<TrackView?>(null) }
                LaunchedEffect(model.catalogVersion, filter) { view = model.core.tracks(filter, TrackOrder.TITLE, false) }
                view?.let { TrackList(model, it) }
            }
        }
    }
}

@Composable
fun SyncLine(model: Model) {
    val s = model.sync
    Row(Modifier.fillMaxWidth().padding(16.dp, 0.dp, 8.dp, 0.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(
            when {
                s.error != null && !s.haveCatalog -> s.error!!
                !s.haveCatalog -> "Loading the library…"
                else -> "${s.albums} albums · ${s.tracks} tracks" + if (s.refreshing) " · updating…" else ""
            },
            Modifier.weight(1f), style = MaterialTheme.typography.labelLarge,
        )
        var open by remember { mutableStateOf(false) }
        TextButton({ open = true }) { Text("Settings") }
        if (open) SettingsDialog(model) { open = false }
    }
}

@Composable
private fun SettingsDialog(model: Model, close: () -> Unit) {
    AlertDialog(
        onDismissRequest = close,
        confirmButton = { TextButton(close) { Text("Done") } },
        dismissButton = { TextButton({ close(); model.logout() }) { Text("Log out") } },
        title = { Text("Settings") },
        text = {
            Column {
                val q = model.quality
                QualityChoice("Quality on Wi-Fi", q.unmetered, q::chooseUnmetered)
                Spacer(Modifier.height(12.dp))
                QualityChoice("Quality on mobile data", q.metered, q::chooseMetered)
                Spacer(Modifier.height(12.dp))
                Text(
                    if (Output.reduction == 0) "Output level: full" else "Output level: −${Output.reduction} dB",
                    style = MaterialTheme.typography.labelLarge,
                )
                // Below the phone's own volume: for when its lowest step is still too loud.
                Slider(
                    value = Output.reduction.toFloat(), onValueChange = { Output.set(it.toInt()) },
                    valueRange = 0f..30f, steps = 29,
                )
            }
        },
    )
}

@Composable
private fun QualityChoice(title: String, chosen: Int, choose: (Int) -> Unit) {
    Text(title, style = MaterialTheme.typography.labelLarge)
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        QUALITIES.forEach { kbps -> FilterChip(chosen == kbps, { choose(kbps) }, { Text(qualityName(kbps)) }) }
    }
}
