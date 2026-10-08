package io.github.allinen.acetate

import android.content.Context
import android.content.Intent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import uniffi.acetate_ffi.CoreException
import uniffi.acetate_ffi.MarkOn

/** Hands text to the system's share sheet. */
fun shareText(context: Context, text: String) {
    val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)
    context.startActivity(Intent.createChooser(send, null).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
}

/**
 * What to share: a link for someone with an account on this server (it opens in their app),
 * or, for a track or album, a link anyone can listen with (it expires after the chosen time).
 */
@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
fun ShareDialog(model: Model, on: MarkOn, id: String, what: String, close: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var days by remember { mutableStateOf<Int?>(7) }
    var making by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = close,
        title = { Text("Share $what") },
        text = {
            Column {
                Text("A link for someone with an account on this server:", style = MaterialTheme.typography.bodySmall)
                TextButton({
                    model.core.openLink(on, id)?.let { shareText(context, it) }
                    close()
                }) { Text("Share link") }
                if (on != MarkOn.ARTIST) {
                    Spacer(Modifier.height(8.dp))
                    Text("A link anyone can listen with, until:", style = MaterialTheme.typography.bodySmall)
                    FlowRow(Modifier.fillMaxWidth().padding(vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        listOf(1 to "1 day", 7 to "7 days", 30 to "30 days", null to "Never").forEach { (d, name) ->
                            FilterChip(days == d, { days = d }, { Text(name) })
                        }
                    }
                    TextButton({
                        making = true
                        scope.launch {
                            try {
                                shareText(context, model.core.makeShare(on, id, days?.toUInt()))
                            } catch (e: CoreException) {
                                model.notice = "Couldn't make a share link: ${e.message}"
                            }
                            close()
                        }
                    }, enabled = !making) { Text(if (making) "Making it…" else "Make a link anyone can use") }
                }
            }
        },
        confirmButton = { TextButton(close) { Text("Cancel") } },
    )
}
