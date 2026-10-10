package io.github.allinen.acetate

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import uniffi.acetate_ffi.ChangeEntry
import uniffi.acetate_ffi.CoreException
import uniffi.acetate_ffi.HistoryEntry
import uniffi.acetate_ffi.MarkOn

/**
 * A track's, album's or artist's revisions, newest first, with what each changed in words,
 * below its page (for those who may see history). In edit mode an old one can be put back:
 * that writes a new revision, the ones in between stay.
 */
@Composable
fun HistorySection(model: Model, on: MarkOn, id: String) {
    val may = remember(model.accountVersion, model.session) { model.core.canViewHistory() }
    if (!may) return
    var entries by remember(id) { mutableStateOf<List<HistoryEntry>?>(null) }
    var failure by remember(id) { mutableStateOf<String?>(null) }
    var reload by remember(id) { mutableIntStateOf(0) }
    var asking by remember(id) { mutableStateOf<HistoryEntry?>(null) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(id, reload, model.connection.offline) {
        if (model.connection.offline) return@LaunchedEffect
        try { entries = model.core.history(on, id); failure = null } catch (e: CoreException) { failure = e.message }
    }
    val canRestore = model.editMode && remember(model.accountVersion, model.session) { model.core.canRestore() }
    Column(Modifier.fillMaxWidth().padding(16.dp, 0.dp, 16.dp, 24.dp)) {
        SectionHeader("History")
        when {
            model.connection.offline && entries == null -> Muted("History needs the server.")
            failure != null -> Muted("Couldn't ask the server: $failure")
            entries == null -> Muted("Asking…")
            entries!!.isEmpty() -> Muted("No revisions you can see.")
        }
        entries?.forEach { e ->
            Revision(e.at, e.title, e.reason, e.lines, if (e.current) "now" else null) {
                if (canRestore && !e.current) TextButton({ asking = e }) { Text("Restore…") }
            }
        }
    }
    asking?.let { e ->
        AlertDialog(
            onDismissRequest = { asking = null },
            title = { Text("Restore this one?") },
            text = { Text("${e.at} · ${e.title}\n\nRestoring writes a new revision with this one's data. The ones after it stay in the history (and can be restored too).") },
            confirmButton = {
                TextButton({
                    asking = null
                    scope.launch {
                        try {
                            model.core.restoreRevision(on, e.id, "")
                            model.notice = "Restored."
                            reload++
                            model.core.startSync()
                        } catch (x: CoreException) {
                            model.notice = "Couldn't restore it: ${x.message}"
                        }
                    }
                }) { Text("Restore it") }
            },
            dismissButton = { TextButton({ asking = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun Muted(text: String) = Text(text, color = Palette.faint, style = MaterialTheme.typography.bodyMedium)

/** One revision: when, how and by whom, why, what it changed. */
@Composable
private fun Revision(whenText: String, title: String, reason: String, lines: List<String>, mark: String?, action: @Composable () -> Unit = {}) {
    Column(Modifier.padding(vertical = 6.dp)) {
        Row {
            Text(whenText, style = MaterialTheme.typography.labelLarge)
            mark?.let { Text("  · $it", color = Palette.accent, style = MaterialTheme.typography.labelLarge) }
        }
        Text(title, color = Palette.faint, style = MaterialTheme.typography.bodySmall)
        if (reason.isNotEmpty()) Text("“$reason”", color = Palette.faint, fontStyle = FontStyle.Italic, style = MaterialTheme.typography.bodySmall)
        lines.forEach { Text(it, Modifier.padding(start = 8.dp), style = MaterialTheme.typography.bodyMedium) }
        action()
    }
}

/** What was changed lately in the library's metadata, by anyone, newest first. A name opens its page. */
@Composable
fun ChangesScreen(model: Model) {
    var imports by remember { mutableStateOf(false) }
    var list by remember { mutableStateOf<List<ChangeEntry>>(emptyList()) }
    var more by remember { mutableStateOf(false) }
    var loading by remember { mutableStateOf(true) }
    var failure by remember { mutableStateOf<String?>(null) }
    var again by remember { mutableIntStateOf(0) }
    val scope = rememberCoroutineScope()
    val may = remember(model.accountVersion, model.session) { model.core.canViewHistory() }
    LaunchedEffect(imports, again) {
        if (!may) return@LaunchedEffect
        loading = true
        try {
            val page = model.core.changes(null, imports)
            list = page.entries; more = page.more; failure = null
        } catch (e: CoreException) { failure = e.message }
        loading = false
    }
    PageScaffold(model, "Changes") { modifier ->
        LazyColumn(modifier) {
            if (!may) item { Column(Modifier.padding(16.dp)) { Muted("Seeing what changed needs the permission to view history.") } }
            else {
                item {
                    Row(Modifier.padding(16.dp, 8.dp)) {
                        FilterChip(imports, { imports = !imports }, { Text("Include imports") })
                        TextButton({ again++ }, enabled = !loading) { Text(if (loading) "Asking…" else "Refresh") }
                    }
                }
                failure?.let { item { Column(Modifier.padding(16.dp)) { Text("Couldn't ask the server: $it", color = Palette.danger) } } }
                if (list.isEmpty() && !loading && failure == null) item { Column(Modifier.padding(16.dp)) { Muted("Nothing has been changed yet.") } }
                items(list.size, key = { list[it].id }) { i ->
                    val c = list[i]
                    Column(Modifier.fillMaxWidth().clickable {
                        when (c.on) {
                            MarkOn.ALBUM -> model.pages.add(Page.Album(c.entityId))
                            MarkOn.ARTIST -> model.pages.add(Page.Artist(c.entityId))
                            MarkOn.TRACK -> model.pages.add(Page.Track(c.entityId))
                        }
                    }.padding(16.dp, 6.dp)) {
                        Text(c.name, maxLines = 1, color = Palette.accent, style = MaterialTheme.typography.titleSmall)
                        Revision(c.at, c.how, c.reason, c.lines, null)
                    }
                }
                if (more) item {
                    TextButton({
                        scope.launch {
                            loading = true
                            try {
                                val page = model.core.changes(list.last().id, imports)
                                list = list + page.entries; more = page.more
                            } catch (e: CoreException) { failure = e.message }
                            loading = false
                        }
                    }, Modifier.padding(8.dp), enabled = !loading) { Text(if (loading) "Asking…" else "Older") }
                }
            }
        }
    }
}

/** What the app said in snackbars lately (the last hundred, newest first); the text can be selected. */
@Composable
fun MessagesScreen(model: Model) {
    val list = remember { model.core.messages() }
    PageScaffold(model, "Earlier messages") { modifier ->
        androidx.compose.foundation.text.selection.SelectionContainer {
            LazyColumn(modifier) {
                if (list.isEmpty()) item { Column(Modifier.padding(16.dp)) { Muted("None yet.") } }
                items(list.size) { i ->
                    val m = list[i]
                    Column(Modifier.fillMaxWidth().padding(16.dp, 6.dp)) {
                        Muted(m.ago)
                        Text(m.text, color = if (m.error) Palette.danger else Palette.text)
                    }
                }
            }
        }
    }
}
