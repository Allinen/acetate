package io.github.allinen.acetate

import androidx.compose.material3.Slider
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
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


@Composable
fun LibraryScreen(model: Model) {
    val tab = model.tab
    // One filter line per tab, in the library's query language (`artist:… year:…`).
    var filters by rememberSaveable { mutableStateOf(listOf("", "", "", "")) }
    Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal))) {
        if (tab != 4) SyncLine(model)
        if (tab < 4) {
            LineField(
                filters[tab], { text -> filters = filters.toMutableList().also { it[tab] = text } }, "Filter",
                Modifier.fillMaxWidth().padding(16.dp, 8.dp),
            )
            // (A playlist's name is plain words: no keys to suggest.)
            if (tab < 3) Suggestions(model, filters[tab]) { line -> filters = filters.toMutableList().also { it[tab] = line } }
        }
        val filter = filters.getOrElse(tab) { "" }
        when (tab) {
            0 -> {
                var view by remember { mutableStateOf<AlbumView?>(null) }
                // A new catalog (or the first) or a new filter asks for the view again.
                LaunchedEffect(model.catalogVersion, filter) { view = model.core.albums(filter, AlbumOrder.TITLE) }
                view?.let { AlbumList(model, it) }
            }
            1 -> {
                var view by remember { mutableStateOf<ArtistView?>(null) }
                LaunchedEffect(model.catalogVersion, filter) { view = model.core.artists(filter, ArtistOrder.RELEVANCE) }
                view?.let { ArtistList(model, it) }
            }
            2 -> {
                var view by remember { mutableStateOf<TrackView?>(null) }
                LaunchedEffect(model.catalogVersion, filter) { view = model.core.tracks(filter, TrackOrder.TITLE, false) }
                view?.let { TrackList(model, it) }
            }
            3 -> PlaylistList(model, filter)
            else -> SearchScreen(model)
        }
    }
}

@Composable
fun SyncLine(model: Model) {
    val s = model.sync
    Row(Modifier.fillMaxWidth().padding(16.dp, 0.dp, 8.dp, 0.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(
            when {
                model.downloads.queued > 0uL -> "↓ ${model.downloads.done} of ${model.downloads.asked}" + if (model.downloadSpeed > 0) " · ${speedText(model.downloadSpeed)}" else if (model.downloads.paused) " · stopped" else " · waiting"
                s.error != null && !s.haveCatalog -> s.error!!
                !s.haveCatalog -> "Loading the library…"
                else -> "${s.albums} albums · ${s.tracks} tracks" + if (s.refreshing) " · updating…" else ""
            },
            Modifier.weight(1f), style = MaterialTheme.typography.labelLarge,
        )
        val mayEdit = remember(model.accountVersion, model.session) { model.core.mayEdit() }
        if (mayEdit) {
            FilterChip(model.editMode, {
                // Editing needs the server (turning it off doesn't).
                if (!model.editMode && model.connection.offline) model.notice = "Offline: editing needs the server."
                else model.editMode = !model.editMode
            }, { Text(if (model.connection.offline && !model.editMode) "Edit (offline)" else "Edit") })
        }
        TextButton({ model.settingsOpen = true }) { Text("Settings") }
        if (model.settingsOpen) SettingsDialog(model) { model.settingsOpen = false }
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
            Column(Modifier.verticalScroll(rememberScrollState())) {
                AccountSection(model)
                Spacer(Modifier.height(12.dp))
                model.core.versionWarning()?.let {
                    Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
                    Spacer(Modifier.height(12.dp))
                }
                if (model.core.canViewHistory()) {
                    TextButton({ close(); model.pages.add(Page.Changes) }) { Text("What changed lately") }
                }
                TextButton({ close(); model.pages.add(Page.Messages) }) { Text("Earlier messages") }
                val q = model.quality
                QualityChoice("Quality on Wi-Fi", q.unmetered, q::chooseUnmetered)
                Spacer(Modifier.height(12.dp))
                QualityChoice("Quality on mobile data", q.metered, q::chooseMetered)
                Spacer(Modifier.height(12.dp))
                DownloadSettings(model)
                Spacer(Modifier.height(8.dp))
                // Lyrics too late: move them sooner (more); too early: later (less).
                Text("Lyrics timing", style = MaterialTheme.typography.titleSmall)
                Text("Bluetooth: ${LyricsTiming.bluetoothMs} ms", style = MaterialTheme.typography.labelLarge)
                Slider(LyricsTiming.bluetoothMs.toFloat(), { LyricsTiming.setBluetooth((it / 50).toInt() * 50) }, valueRange = -500f..2000f)
                Text("Other: ${LyricsTiming.otherMs} ms", style = MaterialTheme.typography.labelLarge)
                Slider(LyricsTiming.otherMs.toFloat(), { LyricsTiming.setOther((it / 50).toInt() * 50) }, valueRange = -500f..2000f)
                Spacer(Modifier.height(8.dp))
                Text(
                    if (Output.reduction == 0) "Output level: full" else "Output level: −${Output.reduction} dB",
                    style = MaterialTheme.typography.labelLarge,
                )
                // Right is louder, as on any volume slider; it only goes down from full, below the phone's own volume.
                Slider(
                    value = (30 - Output.reduction).toFloat(), onValueChange = { Output.set(30 - it.toInt()) },
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
