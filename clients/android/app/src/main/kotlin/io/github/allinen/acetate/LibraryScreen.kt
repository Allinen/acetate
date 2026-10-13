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
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Switch
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
import kotlinx.coroutines.launch
import uniffi.acetate_ffi.CoreException
import uniffi.acetate_ffi.AlbumOrder
import uniffi.acetate_ffi.formatBytes
import uniffi.acetate_ffi.AlbumView
import uniffi.acetate_ffi.ArtistOrder
import uniffi.acetate_ffi.ArtistView
import uniffi.acetate_ffi.TrackOrder
import uniffi.acetate_ffi.TrackView


@Composable
fun LibraryScreen(model: Model) {
    val tab = model.tab
    VersionPrompt(model)
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

/**
 * The server is of another release than this app: say so once (per server version). When the
 * server's is newer, the app can update itself to it (the core downloads and checks it, the
 * system installs it); an older server is only warned about, with the matching app to
 * install by hand. Nothing is fetched until they say so.
 */
@Composable
private fun VersionPrompt(model: Model) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val warning = remember(model.accountVersion, model.session) { model.core.versionWarning() }
    val url = remember(model.accountVersion, model.session) { model.core.versionUpdateUrl() }
    val newer = remember(model.accountVersion, model.session) { model.core.versionUpdate() }
    // Not the same API version: the server turns the app away, so this can't be put off.
    val blocked = remember(model.accountVersion, model.session) { model.core.versionBlocked() }
    var dismissed by rememberSaveable { mutableStateOf<String?>(null) }
    var updating by remember { mutableStateOf(false) }
    var failure by remember { mutableStateOf<String?>(null) }
    var progress by remember { mutableStateOf(uniffi.acetate_ffi.UpdateProgress(0uL, 0uL)) }
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    LaunchedEffect(updating) {
        while (updating) {
            progress = model.core.updateProgress()
            kotlinx.coroutines.delay(200)
        }
    }
    if (warning == null || (!blocked && !updating && dismissed == warning)) return
    val text = when {
        updating -> if (progress.total > 0uL) "Downloading Acetate $newer… ${formatBytes(progress.done)} of ${formatBytes(progress.total)}" else "Downloading Acetate $newer…"
        newer != null -> "The server is on Acetate $newer, newer than this app (${BuildConfig.VERSION_NAME}). Update the app to match it." + (failure?.let { "\n\nThe update failed: $it" } ?: "")
        else -> warning
    }
    AlertDialog(
        onDismissRequest = { if (!blocked && !updating) dismissed = warning },
        title = { Text(if (blocked) "Update needed" else if (newer != null) "Update available" else "A different version") },
        text = { Text(text) },
        confirmButton = {
            if (newer != null) TextButton({
                updating = true
                failure = null
                scope.launch {
                    try {
                        AppUpdate.install(context, model.core.downloadUpdate())
                        // (The installer's confirmation is up; if it's declined, this can be asked again.)
                        dismissed = warning
                    } catch (e: CoreException) {
                        failure = e.message
                    } catch (e: Exception) {
                        failure = e.message ?: "The installer wouldn't take it."
                    }
                    updating = false
                }
            }, enabled = !updating) { Text(if (failure != null) "Try again" else "Update") }
            else if (url != null) TextButton({
                dismissed = warning
                context.startActivity(android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(url)))
            }) { Text("Download") }
        },
        dismissButton = {
            if (blocked) TextButton({ model.logout() }) { Text("Log out") }
            else TextButton({ dismissed = warning }, enabled = !updating) { Text("Later") }
        },
    )
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
                Spacer(Modifier.height(12.dp))
                EqualizerSection()
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

@Composable
private fun EqualizerSection() {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text("Equalizer", style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
        TextButton({ Equalizer.flatten() }, enabled = Equalizer.gains.any { it != 0 }) { Text("Flat") }
        Switch(Equalizer.enabled, { Equalizer.enable(it) })
    }
    Equalizer.BANDS.forEachIndexed { band, hz ->
        val name = if (hz >= 1000f) "${(hz / 1000f).toInt()} kHz" else "${hz.toInt()} Hz"
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(name, Modifier.width(56.dp), style = MaterialTheme.typography.labelMedium)
            Slider(
                Equalizer.gains[band].toFloat(), { Equalizer.set(band, it.toInt()) },
                Modifier.weight(1f), enabled = Equalizer.enabled,
                valueRange = -Equalizer.RANGE_DB.toFloat()..Equalizer.RANGE_DB.toFloat(), steps = 2 * Equalizer.RANGE_DB - 1,
            )
            Text("%+d".format(Equalizer.gains[band]), Modifier.width(32.dp), style = MaterialTheme.typography.labelMedium)
        }
    }
    val headroom = Equalizer.headroomDb()
    if (Equalizer.enabled && headroom > 0) {
        Text("The sound is lowered by $headroom dB to make room for the boosts, so they can't clip.", style = MaterialTheme.typography.bodySmall)
    }
}
