package io.github.allinen.acetate

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
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
import uniffi.acetate_ffi.PinKind

/**
 * Keeps something on the device: "Download", then "Stop" while files come (what came stays;
 * "Resume" goes on), then "Downloaded ✓", which asks before taking the files away.
 */
@Composable
fun DownloadButton(model: Model, kind: PinKind, id: String) {
    val pinned = remember(kind, id, model.downloadsVersion) { model.core.isDownloading(kind, id) }
    val d = model.downloads
    var confirm by remember { mutableStateOf(false) }
    val coming = d.queued > 0uL
    TextButton({
        when {
            !pinned -> model.core.download(kind, id, true)
            coming && !d.paused -> model.core.pauseDownloads()
            coming -> model.core.resumeDownloads()
            else -> confirm = true
        }
    }) {
        Text(
            when {
                !pinned -> "Download"
                coming && d.paused -> "Resume"
                coming -> "Stop"
                else -> "Downloaded ✓"
            },
        )
    }
    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("Remove this download?") },
            text = { Text("Its files are deleted from the phone (anything kept for something else stays). The music on the server isn't touched.") },
            confirmButton = { TextButton({ model.core.download(kind, id, false); confirm = false }) { Text("Remove") } },
            dismissButton = { TextButton({ confirm = false }) { Text("Cancel") } },
        )
    }
}

/** Whether a track's file is on the device (a mark in its row). */
@Composable
fun rememberDownloaded(model: Model, track: String): Boolean =
    remember(track, model.downloadsVersion, model.catalogVersion) { model.core.isDownloaded(track) }

/** The settings for downloads: quality, mobile data, what's kept, and taking it all away. */
@Composable
fun DownloadSettings(model: Model) {
    val q = model.quality
    val d = model.downloads
    var confirm by remember { mutableStateOf(false) }
    Text("Downloads", style = MaterialTheme.typography.titleSmall)
    Text("Quality", style = MaterialTheme.typography.labelLarge)
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        QUALITIES.forEach { kbps -> FilterChip(q.download == kbps, { q.chooseDownload(kbps) }, { Text(qualityName(kbps)) }) }
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text("Download on mobile data", Modifier.weight(1f))
        Switch(q.downloadOnMobile, { q.chooseDownloadOnMobile(it) })
    }
    Text(
        "${d.files} files · ${megabytes(d.bytes.toLong())}" + if (d.queued > 0uL) " · ${d.queued} coming" else "",
        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    if (d.failed > 0uL) Text("${d.failed} couldn't be fetched", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
    if (d.files > 0uL || d.pins.isNotEmpty()) TextButton({ confirm = true }) { Text("Remove all downloads") }
    Spacer(Modifier.height(4.dp))
    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("Remove all downloads?") },
            text = { Text("The ${d.files} files on this device are deleted (the music on the server isn't touched). Nothing stays kept offline.") },
            confirmButton = { TextButton({ model.core.removeDownloads(); confirm = false }) { Text("Remove") } },
            dismissButton = { TextButton({ confirm = false }) { Text("Cancel") } },
        )
    }
}

private fun megabytes(bytes: Long): String =
    if (bytes >= 1_000_000_000) "%.1f GB".format(bytes / 1e9) else "%d MB".format(bytes / 1_000_000)
