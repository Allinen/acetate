package io.github.allinen.acetate

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** The settings page's tabs, like the desktop's (Settings, `Ctrl+,`). */
private enum class SettingsTab(val title: String) {
    Sound("Sound"),
    Downloads("Downloads"),
    Appearance("Appearance"),
    Account("Account"),
}

/** Settings: a page with a tab for each kind of setting; back returns to where it was opened. */
@Composable
fun SettingsScreen(model: Model) {
    var tab by rememberSaveable { mutableStateOf(SettingsTab.Sound.name) }
    val shown = SettingsTab.valueOf(tab)
    PageScaffold(model, "Settings") { modifier ->
        Column(modifier) {
            Row(
                Modifier.horizontalScroll(rememberScrollState()).padding(16.dp, 4.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                SettingsTab.entries.forEach { FilterChip(it == shown, { tab = it.name }, { Text(it.title) }) }
            }
            Column(Modifier.verticalScroll(rememberScrollState(), enabled = true).padding(16.dp, 8.dp)) {
                when (shown) {
                    SettingsTab.Sound -> SoundTab(model)
                    SettingsTab.Downloads -> DownloadSettings(model)
                    SettingsTab.Appearance -> AppearanceTab()
                    SettingsTab.Account -> AccountTab(model)
                }
            }
        }
    }
}

@Composable
private fun SoundTab(model: Model) {
    val q = model.quality
    Text("Streaming quality", style = MaterialTheme.typography.titleSmall)
    QualityChoice("On Wi-Fi", q.unmetered, q::chooseUnmetered)
    Spacer(Modifier.height(12.dp))
    QualityChoice("On mobile data", q.metered, q::chooseMetered)
    Spacer(Modifier.height(20.dp))

    Text(
        if (Output.reduction == 0) "Output level: full" else "Output level: −${Output.reduction} dB",
        style = MaterialTheme.typography.titleSmall,
    )
    // Right is louder, as on any volume slider; it only goes down from full, below the phone's own volume.
    Slider(
        value = (30 - Output.reduction).toFloat(), onValueChange = { Output.set(30 - it.toInt()) },
        valueRange = 0f..30f, steps = 29,
    )
    Spacer(Modifier.height(12.dp))

    // Lyrics too late: move them sooner (more); too early: later (less).
    Text("Lyrics timing", style = MaterialTheme.typography.titleSmall)
    Text("Bluetooth: ${LyricsTiming.bluetoothMs} ms", style = MaterialTheme.typography.labelLarge)
    Slider(LyricsTiming.bluetoothMs.toFloat(), { LyricsTiming.setBluetooth((it / 50).toInt() * 50) }, valueRange = -500f..2000f)
    Text("Other: ${LyricsTiming.otherMs} ms", style = MaterialTheme.typography.labelLarge)
    Slider(LyricsTiming.otherMs.toFloat(), { LyricsTiming.setOther((it / 50).toInt() * 50) }, valueRange = -500f..2000f)
    Spacer(Modifier.height(20.dp))

    EqualizerSection()
}

@Composable
private fun AccountTab(model: Model) {
    AccountSection(model)
    Spacer(Modifier.height(12.dp))
    model.core.versionWarning()?.let {
        Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
        Spacer(Modifier.height(12.dp))
    }
    if (model.core.canViewHistory()) {
        TextButton({ model.pages.add(Page.Changes) }) { Text("What changed lately") }
    }
    TextButton({ model.pages.add(Page.Messages) }) { Text("Earlier messages") }
    TextButton({ model.logout() }) { Text("Log out") }
}

@Composable
fun QualityChoice(title: String, chosen: Int, choose: (Int) -> Unit) {
    Text(title, style = MaterialTheme.typography.labelLarge)
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        QUALITIES.forEach { kbps -> FilterChip(chosen == kbps, { choose(kbps) }, { Text(qualityName(kbps)) }) }
    }
}


@Composable
fun EqualizerSection() {
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
