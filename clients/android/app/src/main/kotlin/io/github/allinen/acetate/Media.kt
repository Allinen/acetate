package io.github.allinen.acetate

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.acetate_ffi.CoreException
import uniffi.acetate_ffi.CoverInfo
import uniffi.acetate_ffi.MarkOn
import uniffi.acetate_ffi.NoteInfo

/** An image stored on the phone, as bytes and a file name (null: it couldn't be read, or is too big). */
suspend fun readImage(context: Context, uri: Uri): Pair<ByteArray, String>? = withContext(Dispatchers.IO) {
    try {
        val name = context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        } ?: "image"
        val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: return@withContext null
        if (bytes.size > 30 * 1024 * 1024) null else bytes to name
    } catch (_: Exception) {
        null
    }
}

/** Picks an image from the phone's storage (the system picker: no permission needed) and hands it over. */
@Composable
fun rememberImagePicker(model: Model, onImage: suspend (ByteArray, String) -> Unit): () -> Unit {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        if (uri != null) scope.launch {
            val image = readImage(context, uri)
            if (image == null) model.notice = "That image couldn't be read (or it's over 30 MB)." else onImage(image.first, image.second)
        }
    }
    return { launcher.launch("image/*") }
}

/** An album's covers, in order, the first the main one: each change is sent at once. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun CoversScreen(model: Model, album: String) {
    var covers by remember(album) { mutableStateOf<List<CoverInfo>?>(null) }
    var problem by remember(album) { mutableStateOf<String?>(null) }
    var busy by remember(album) { mutableStateOf(false) }
    var trashing by remember(album) { mutableStateOf<CoverInfo?>(null) }
    var reload by remember(album) { mutableIntStateOf(0) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(album, reload) {
        try { covers = model.core.albumCovers(album); problem = null } catch (e: CoreException.Failed) { problem = e.reason }
    }

    // One change: sent, then the list read again and the library caught up.
    fun change(done: String, work: suspend () -> Unit) {
        busy = true
        scope.launch {
            try {
                work()
                model.core.startSync()
                reload++
                model.notice = done
                problem = null
            } catch (e: CoreException.Failed) {
                problem = e.reason
            }
            busy = false
        }
    }
    val pick = rememberImagePicker(model) { bytes, name -> change("Added.") { model.core.addAlbumCover(album, bytes, name) } }

    BackHandler { model.form = null }
    Surface(Modifier.fillMaxSize(), color = Palette.bg) {
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Bottom + WindowInsetsSides.Horizontal))) {
            Row(Modifier.fillMaxWidth().padding(8.dp, 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Covers", Modifier.weight(1f).padding(start = 16.dp), style = MaterialTheme.typography.titleMedium)
                Button({ model.form = null }) { Text("Done") }
            }
            problem?.let { Text(it, Modifier.padding(16.dp, 4.dp), color = Palette.danger) }
            val list = covers
            LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
                if (list == null) item { Text("Loading…", Modifier.padding(16.dp), color = Palette.muted) }
                else if (list.isEmpty()) item { Text("No covers of its own.", Modifier.padding(16.dp), color = Palette.muted) }
                items(list ?: emptyList(), key = { it.id }) { c ->
                    val n = list!!.indexOf(c)
                    Row(Modifier.fillMaxWidth().padding(16.dp, 8.dp), verticalAlignment = Alignment.Top) {
                        Cover(model, c.key, 112)
                        Column(Modifier.weight(1f).padding(start = 12.dp)) {
                            Text(if (n == 0) "Main" else "#${n + 1}", color = if (n == 0) Palette.accent else Palette.text, style = MaterialTheme.typography.titleSmall)
                            Text(
                                listOfNotNull(if (c.width != null && c.height != null) "${c.width}×${c.height}" else null, c.source).joinToString(" · "),
                                color = Palette.muted, style = MaterialTheme.typography.bodySmall,
                            )
                            FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                                fun move(to: Int) {
                                    val ids = list.map { it.id }.toMutableList()
                                    ids.add(to, ids.removeAt(n))
                                    change("Reordered.") { model.core.orderAlbumCovers(album, ids) }
                                }
                                if (n > 0) TextButton({ move(0) }, enabled = !busy) { Text("Make main") }
                                if (n > 0) TextButton({ move(n - 1) }, enabled = !busy) { Text("▲") }
                                if (n < list.size - 1) TextButton({ move(n + 1) }, enabled = !busy) { Text("▼") }
                                TextButton({ change("Put on its tracks.") { model.core.coverOnTracks(album, c.id) } }, enabled = !busy) { Text("On all its tracks") }
                                TextButton({ trashing = c }, enabled = !busy) { Text("Trash", color = Palette.danger) }
                            }
                        }
                    }
                }
                item {
                    OutlinedButton(pick, Modifier.padding(16.dp), enabled = !busy) { Text("Add an image from the phone…") }
                    if (busy) Text("Saving…", Modifier.padding(16.dp, 0.dp), color = Palette.muted)
                }
            }
        }
    }
    trashing?.let { c ->
        AlertDialog(
            onDismissRequest = { trashing = null },
            title = { Text("Move this cover to the trash?") },
            text = { Text("The server keeps it there. The image stays wherever else it's used.") },
            confirmButton = { TextButton({ trashing = null; change("Moved to the trash.") { model.core.trashCover(c.id) } }) { Text("Move it") } },
            dismissButton = { TextButton({ trashing = null }) { Text("Cancel") } },
        )
    }
}

/** The notes on a track, album or artist, below its page; in edit mode they can be written, changed and taken off. */
@Composable
fun NotesSection(model: Model, on: MarkOn, id: String) {
    var notes by remember(id) { mutableStateOf<List<NoteInfo>>(emptyList()) }
    var reload by remember(id) { mutableIntStateOf(0) }
    var editing by remember(id) { mutableStateOf<Pair<NoteInfo?, Boolean>?>(null) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(id, reload) {
        try { notes = model.core.notes(on, id) } catch (_: CoreException) {}
    }
    val shown = notes.filter { !it.hidden || model.editMode }
    if (shown.isEmpty() && !model.editMode) return
    Column(Modifier.fillMaxWidth().padding(16.dp, 0.dp)) {
        SectionHeader("Notes")
        shown.forEach { n ->
            Column(Modifier.padding(vertical = 6.dp)) {
                if (n.title.isNotEmpty()) Text(n.title, style = MaterialTheme.typography.titleSmall, color = if (n.hidden) Palette.faint else Palette.text)
                Text(n.body, color = if (n.hidden) Palette.faint else Palette.text, style = MaterialTheme.typography.bodyMedium)
                Text(listOfNotNull(n.updatedBy, n.updatedAt.take(10)).joinToString(" · "), color = Palette.faint, style = MaterialTheme.typography.labelSmall)
                if (model.editMode) Row {
                    TextButton({ editing = n to false }) { Text("Edit") }
                    TextButton({
                        scope.launch {
                            try { model.core.hideNote(n.id, !n.hidden); reload++ } catch (e: CoreException.Failed) { model.notice = e.reason }
                        }
                    }) { Text(if (n.hidden) "Bring back" else "Take off") }
                }
            }
        }
        if (model.editMode) TextButton({ editing = null to true }) { Text("Add a note") }
    }
    editing?.let { (note, _) ->
        NoteDialog(model, on, id, note, { editing = null }) { editing = null; reload++ }
    }
}

@Composable
private fun NoteDialog(model: Model, on: MarkOn, id: String, note: NoteInfo?, close: () -> Unit, saved: () -> Unit) {
    var title by remember { mutableStateOf(note?.title ?: "") }
    var body by remember { mutableStateOf(note?.body ?: "") }
    var reason by remember { mutableStateOf("") }
    var problem by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    AlertDialog(
        onDismissRequest = { if (!saving) close() },
        title = { Text(if (note == null) "New note" else "Edit note") },
        text = {
            Column {
                FocusedField(title, { title = it }, Modifier.fillMaxWidth(), { Text("Title (optional)") })
                OutlinedTextField(body, { body = it }, Modifier.fillMaxWidth().padding(top = 8.dp).height(180.dp), label = { Text("The note") })
                OutlinedTextField(reason, { reason = it }, Modifier.fillMaxWidth().padding(top = 8.dp), label = { Text("Why (optional)") }, singleLine = true)
                problem?.let { Text(it, color = Palette.danger, modifier = Modifier.padding(top = 8.dp)) }
            }
        },
        confirmButton = {
            TextButton({
                saving = true
                scope.launch {
                    try {
                        model.core.saveNote(on, id, note, title, body, reason)
                        saved()
                    } catch (e: CoreException.Failed) {
                        problem = if (e.code == "stale") "Someone changed this note meanwhile. Close it and open it again." else e.reason
                    }
                    saving = false
                }
            }, enabled = !saving && (title.isNotBlank() || body.isNotBlank())) { Text(if (saving) "Saving…" else "Save") }
        },
        dismissButton = { TextButton(close, enabled = !saving) { Text("Cancel") } },
    )
}

/** A single-line text field that takes the cursor (and the keyboard) when it appears: for dialogs. */
@Composable
fun FocusedField(value: String, onChange: (String) -> Unit, modifier: Modifier = Modifier, label: (@Composable () -> Unit)? = null) {
    val focus = remember { androidx.compose.ui.focus.FocusRequester() }
    LaunchedEffect(Unit) { try { focus.requestFocus() } catch (_: IllegalStateException) {} }
    OutlinedTextField(value, onChange, modifier.then(Modifier.focusRequester(focus)), label = label, singleLine = true)
}
