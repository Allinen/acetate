package io.github.allinen.acetate

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import uniffi.acetate_ffi.AddressAnswer
import uniffi.acetate_ffi.CoreException

/** Settings: the server and who is logged in; the server's address can change (to the same server). */
@Composable
fun AccountSection(model: Model) {
    val session = model.session ?: return
    var editing by remember { mutableStateOf(false) }
    var address by remember { mutableStateOf(session.server) }
    // What the address typed answered; None while it is asked.
    var answer by remember { mutableStateOf<AddressAnswer?>(null) }
    var asking by remember { mutableStateOf(false) }
    var failure by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    Text("Account", style = MaterialTheme.typography.titleSmall)
    Text("Server: ${session.server}", style = MaterialTheme.typography.bodyMedium)
    Text("Logged in as ${session.username}", style = MaterialTheme.typography.bodyMedium)
    if (!editing) {
        TextButton({ editing = true; address = session.server; answer = null; failure = null }) { Text("Change address") }
        return
    }
    FocusedField(address, { address = it; answer = null; failure = null }, label = { Text("Address") })
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (answer?.same == true) {
            Button({
                scope.launch {
                    try {
                        model.core.changeAddress(address.trim())
                        model.refreshSession()
                        model.core.tryNow()
                        model.notice = "The address is changed. Nothing else was: the library, downloads and queue are the same."
                        editing = false
                    } catch (e: CoreException) { failure = e.message }
                }
            }) { Text("Use this address") }
        } else {
            Button({
                asking = true; answer = null
                scope.launch { answer = model.core.checkAddress(address.trim().trimEnd('/')); asking = false }
            }, enabled = address.isNotBlank() && !asking) { Text(if (asking) "Asking…" else "Check") }
        }
        OutlinedButton({ editing = false }) { Text("Cancel") }
    }
    answer?.let { Text(if (it.same) "The same server, and your login works there." else it.text, color = if (it.same) Palette.text else Palette.danger) }
    failure?.let { Text(it, color = Palette.danger) }
    Text(
        "Only the same server is accepted, found by its identity: the library, downloads and queue stay. A different server needs a log out and a log in.",
        style = MaterialTheme.typography.bodySmall, color = Palette.faint,
    )
}
