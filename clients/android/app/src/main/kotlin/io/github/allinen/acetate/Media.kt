package io.github.allinen.acetate

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
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

/** A cover in the screen: one the album has, or an image added here (not uploaded until Save). */
private class CoverDraft(val info: CoverInfo?, val bytes: ByteArray?, val name: String?) {
    /** Will go to the trash. */
    var trash by mutableStateOf(false)

    /** Will be put on the album's tracks. */
    var onTracks by mutableStateOf(false)
}

/**
 * An album's covers, in order, the first the main one. Nothing is sent until Save: making a
 * cover main, moving it, trashing it, putting it on the tracks and adding an image from the
 * phone are staged here.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun CoversScreen(model: Model, album: String) {
    val items = remember(album) { mutableStateListOf<CoverDraft>() }
    var original by remember(album) { mutableStateOf<List<String>?>(null) }
    var problem by remember(album) { mutableStateOf<String?>(null) }
    var saving by remember(album) { mutableStateOf(false) }
    var leaving by remember(album) { mutableStateOf(false) }
    var load by remember(album) { mutableIntStateOf(0) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(album, load) {
        try {
            val covers = model.core.albumCovers(album)
            items.clear()
            items.addAll(covers.map { CoverDraft(it, null, null) })
            original = covers.map { it.id }
        } catch (e: CoreException.Failed) { problem = e.reason }
    }
    val pick = rememberImagePicker(model) { bytes, name -> items.add(CoverDraft(null, bytes, name)) }
    val changed = original != null && (items.map { it.info?.id } != original || items.any { it.trash || it.onTracks })

    fun close() { model.form = null }

    // Sends it all: the new images first, then the trash, the order, and the tracks' covers.
    fun save() {
        saving = true
        problem = null
        scope.launch {
            try {
                val before = original.orEmpty().toSet()
                val adding = items.filter { it.info == null && !it.trash }
                adding.forEach { model.core.addAlbumCover(album, it.bytes!!, it.name ?: "image") }
                // The server puts them last, in the order they came.
                val made = if (adding.isEmpty()) emptyList() else model.core.albumCovers(album).map { it.id }.filter { it !in before }
                val idOf = { c: CoverDraft -> c.info?.id ?: made.getOrNull(adding.indexOf(c)) }
                items.filter { it.trash && it.info != null }.forEach { model.core.trashCover(it.info!!.id) }
                val order = items.filter { !it.trash }.mapNotNull(idOf)
                if (order != original.orEmpty().filter { id -> items.none { it.trash && it.info?.id == id } }) model.core.orderAlbumCovers(album, order)
                items.filter { it.onTracks && !it.trash }.mapNotNull(idOf).forEach { model.core.coverOnTracks(album, it) }
                model.core.startSync()
                model.notice = "Saved."
                close()
            } catch (e: CoreException.Failed) {
                problem = e.reason
                load++
            }
            saving = false
        }
    }

    BackHandler { if (changed && !saving) leaving = true else close() }
    Surface(Modifier.fillMaxSize(), color = Palette.bg) {
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Bottom + WindowInsetsSides.Horizontal))) {
            Row(Modifier.fillMaxWidth().padding(8.dp, 4.dp), verticalAlignment = Alignment.CenterVertically) {
                TextButton({ if (changed && !saving) leaving = true else close() }) { Text("Cancel") }
                Text("Covers", Modifier.weight(1f).padding(horizontal = 4.dp), style = MaterialTheme.typography.titleMedium)
                Button(::save, enabled = changed && !saving) { Text(if (saving) "Saving…" else "Save") }
            }
            problem?.let { Text(it, Modifier.padding(16.dp, 4.dp), color = Palette.danger) }
            LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
                if (original == null) item { Text("Loading…", Modifier.padding(16.dp), color = Palette.muted) }
                else if (items.isEmpty()) item { Text("No covers of its own.", Modifier.padding(16.dp), color = Palette.muted) }
                items(items.size, key = { System.identityHashCode(items[it]) }) { i ->
                    val c = items[i]
                    // Its place among the covers that stay.
                    val place = items.take(i).count { !it.trash }
                    val last = items.indexOfLast { !it.trash }
                    fun move(to: Int) { items.add(to, items.removeAt(i)) }
                    Row(Modifier.fillMaxWidth().padding(16.dp, 8.dp), verticalAlignment = Alignment.Top) {
                        Box(Modifier.alpha(if (c.trash) 0.35f else 1f)) {
                            if (c.info != null) Cover(model, c.info.key, 112)
                            else {
                                val bitmap = remember(c) {
                                    val bounds = android.graphics.BitmapFactory.Options().apply { inJustDecodeBounds = true }
                                    android.graphics.BitmapFactory.decodeByteArray(c.bytes, 0, c.bytes!!.size, bounds)
                                    val sample = maxOf(1, maxOf(bounds.outWidth, bounds.outHeight) / 400)
                                    android.graphics.BitmapFactory.decodeByteArray(c.bytes, 0, c.bytes.size, android.graphics.BitmapFactory.Options().apply { inSampleSize = sample })?.asImageBitmap()
                                }
                                if (bitmap != null) Image(bitmap, null, Modifier.size(112.dp).clip(RoundedCornerShape(4.dp)), contentScale = ContentScale.Crop)
                                else Box(Modifier.size(112.dp).background(Palette.surfaceHi))
                            }
                        }
                        Column(Modifier.weight(1f).padding(start = 12.dp)) {
                            Text(
                                if (c.trash) "Goes to the trash" else if (place == 0) "Main" else "#${place + 1}",
                                color = if (c.trash) Palette.danger else if (place == 0) Palette.accent else Palette.text, style = MaterialTheme.typography.titleSmall,
                            )
                            Text(
                                if (c.info == null) "new: ${c.name}" else listOfNotNull(if (c.info.width != null && c.info.height != null) "${c.info.width}×${c.info.height}" else null, c.info.source).joinToString(" · "),
                                color = Palette.muted, style = MaterialTheme.typography.bodySmall, maxLines = 1,
                            )
                            if (c.onTracks && !c.trash) Text("goes on all its tracks", color = Palette.accent, style = MaterialTheme.typography.labelSmall)
                            FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                                if (c.trash) {
                                    TextButton({ c.trash = false }, enabled = !saving) { Text("Keep it") }
                                } else {
                                    if (place > 0) TextButton({ move(0) }, enabled = !saving) { Text("Make main") }
                                    if (place > 0) TextButton({ move(items.take(i).indexOfLast { !it.trash }) }, enabled = !saving) { Text("▲") }
                                    if (i < last) TextButton({ move(items.drop(i + 1).indexOfFirst { !it.trash } + i + 1) }, enabled = !saving) { Text("▼") }
                                    TextButton({ c.onTracks = !c.onTracks }, enabled = !saving) { Text(if (c.onTracks) "Not on its tracks" else "On all its tracks") }
                                    TextButton({ if (c.info == null) items.remove(c) else c.trash = true }, enabled = !saving) { Text(if (c.info == null) "Remove" else "Trash", color = Palette.danger) }
                                }
                            }
                        }
                    }
                }
                item {
                    OutlinedButton(pick, Modifier.padding(16.dp), enabled = !saving) { Text("Add an image from the phone…") }
                    Text("Nothing is changed until Save. A trashed cover is kept by the server.", Modifier.padding(16.dp, 0.dp), color = Palette.faint, style = MaterialTheme.typography.labelSmall)
                }
            }
        }
    }
    if (leaving) AlertDialog(
        onDismissRequest = { leaving = false },
        title = { Text("Discard the changes?") },
        text = { Text("Nothing has been saved.") },
        confirmButton = { TextButton({ leaving = false; close() }) { Text("Discard") } },
        dismissButton = { TextButton({ leaving = false }) { Text("Keep editing") } },
    )
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
