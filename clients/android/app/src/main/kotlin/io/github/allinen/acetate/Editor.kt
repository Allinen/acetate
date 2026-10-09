package io.github.allinen.acetate

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import uniffi.acetate_ffi.CoreException
import uniffi.acetate_ffi.Editor
import uniffi.acetate_ffi.EditorChoice
import uniffi.acetate_ffi.EditorFinish
import uniffi.acetate_ffi.EditorFlag
import uniffi.acetate_ffi.EditorRow
import uniffi.acetate_ffi.EditorState

/** What to start a line with, when nothing is typed yet. */
private val starters = listOf("title:", "artist:", "feat:", "album:", "date:", "status:", "genre:", "comment:")

/** A value as the line wants it: spaces and colons are fine (a value runs to the next
 *  `|` or modifier key), only a `|` needs quotes. */
private fun quoted(value: String): String =
    if (value.any { it == '|' || it == '"' }) "\"" + value.replace("\"", "'") + "\"" else value

/** The line that starts changing a row (its key and its value now). */
private fun lineFor(row: EditorRow): String? = when (row.kind) {
    "title" -> "title:" + quoted(row.value)
    "date" -> "date:" + (if (row.value == "none" || row.value.isBlank()) "" else row.value)
    "status" -> "status:"
    "genres" -> "genre:"
    "comment" -> "comment:" + quoted(if (row.value == "none") "" else row.value)
    "credit" -> "artist:" + quoted(row.value.substringBefore(" (").substringBefore(" as ")) + " role:"
    "add_artist" -> "artist:"
    "add_album", "album" -> "album:"
    "add_related", "related" -> "related:"
    else -> null
}

/** The key a row's value is taken off with (a credit's artist, an album). */
private fun removeFor(row: EditorRow): String? = when (row.kind) {
    "credit" -> "-artist:" + quoted(row.value.substringBefore(" (").substringBefore(" as ").removeSuffix(" (new)"))
    "album" -> "-album:" + quoted(row.value.substringBefore(" #").removeSuffix(" (new)"))
    "related" -> "-related:" + quoted(row.value.substringAfter("“").substringBefore("”"))
    else -> null
}

/**
 * The editor: over everything, one track's fields as the staged changes leave them, the
 * changes as chips, what the server's preview says, and the line to type at the bottom
 * (with choices over the keyboard). Every change is a line, whether a row's tap, a chip or
 * typing wrote it; the core says what it means.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun EditorScreen(model: Model, editor: Editor) {
    var state by remember(editor) { mutableStateOf(editor.state()) }
    var line by remember(editor) { mutableStateOf("") }
    var problem by remember(editor) { mutableStateOf<String?>(null) }
    var reason by remember(editor) { mutableStateOf("") }
    var choices by remember(editor) { mutableStateOf<List<EditorChoice>>(emptyList()) }
    var finish by remember(editor) { mutableStateOf<EditorFinish?>(null) }
    var leaving by remember(editor) { mutableStateOf(false) }

    fun refresh() { state = editor.state() }

    // A line put in the field for the person to finish: the cursor goes there (and the keyboard).
    val focus = remember(editor) { FocusRequester() }
    var focusTick by remember(editor) { mutableStateOf(0) }
    LaunchedEffect(focusTick) { if (focusTick > 0) try { focus.requestFocus() } catch (_: IllegalStateException) {} }
    fun fill(text: String) {
        line = text
        focusTick++
    }

    fun close() {
        editor.discard()
        model.editor = null
    }

    fun tryClose() {
        if (editor.hasChanges() && finish == null) leaving = true else close()
    }

    fun run(text: String) {
        try {
            editor.run(text)
            line = ""
            problem = null
        } catch (e: CoreException.Failed) {
            problem = e.reason
        }
        refresh()
    }

    // The background work moved (loaded, previewed, applied).
    LaunchedEffect(model.editorVersion) {
        refresh()
        val done = editor.takeFinish() ?: return@LaunchedEffect
        if (done.failed == 0u) {
            model.undoOffer = UndoOffer(editor, if (done.changed == 1u) "Saved." else "Saved ${done.changed} tracks.", done.canUndo)
            model.core.startSync()
            close()
        } else {
            finish = done
            model.core.startSync()
        }
    }
    // The choices for the line, found as it's typed (and again when what was asked comes).
    LaunchedEffect(line, state.chips.size, model.editorVersion) {
        choices = if (line.isBlank()) emptyList() else editor.choices(line).choices
    }

    BackHandler { tryClose() }
    Surface(Modifier.fillMaxSize(), color = Palette.bg) {
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal)).imePadding()) {
            Row(Modifier.fillMaxWidth().padding(8.dp, 4.dp), verticalAlignment = Alignment.CenterVertically) {
                TextButton({ tryClose() }) { Text(if (finish != null) "Close" else "Cancel") }
                Text(
                    state.title, Modifier.weight(1f).padding(horizontal = 4.dp), maxLines = 1, overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.titleMedium,
                )
                val working = state.phase != "editing"
                Button(
                    {
                        // A reason goes in as a change of its own, kept with the revision.
                        if (reason.isNotBlank() && state.chips.none { it.text.startsWith("why:") }) {
                            try { editor.run("why:" + quoted(reason.trim())) } catch (_: CoreException) {}
                        }
                        try {
                            editor.apply()
                            problem = null
                        } catch (e: CoreException.Failed) {
                            problem = e.reason
                        }
                        refresh()
                    },
                    enabled = state.chips.isNotEmpty() && !working && state.refused == 0u && finish == null,
                ) { Text(if (state.phase == "applying") "Saving…" else "Save") }
            }

            LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(bottom = 8.dp)) {
                item {
                    val note = when {
                        state.phase == "applying" -> "Changing… ${state.done} of ${state.total}"
                        state.loading -> "Loading the track…"
                        state.chips.isNotEmpty() && state.checking -> "Checking it with the server…"
                        else -> null
                    }
                    if (note != null) Text(note, Modifier.padding(16.dp, 4.dp), color = Palette.muted, style = MaterialTheme.typography.labelLarge)
                }
                items(state.rows.size, key = { "r$it" }) { i ->
                    RowItem(state.rows[i], { run(it) }, { fill(it) })
                }
                if (state.chips.isNotEmpty()) item {
                    SectionHeader("Changes")
                    FlowRow(Modifier.padding(16.dp, 0.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        state.chips.forEachIndexed { i, chip ->
                            Row(
                                Modifier.clip(RoundedCornerShape(50)).border(1.dp, if (chip.warning != null) Palette.danger else Palette.line, RoundedCornerShape(50))
                                    .clickable { editor.unstage(i.toUInt())?.let { fill(it) }; refresh() }
                                    .padding(12.dp, 6.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Text(chip.label, style = MaterialTheme.typography.labelLarge, color = Palette.text)
                                Text("  ✕", color = Palette.muted)
                            }
                        }
                    }
                    Text("Tap one to take it back to the line.", Modifier.padding(16.dp, 2.dp), color = Palette.faint, style = MaterialTheme.typography.labelSmall)
                }
                item { Preview(state) }
                if (state.chips.isNotEmpty()) item {
                    OutlinedTextField(
                        reason, { reason = it }, Modifier.fillMaxWidth().padding(16.dp, 8.dp),
                        label = { Text("Why (optional, kept in the history)") }, singleLine = true,
                    )
                }
                finish?.let { f ->
                    item {
                        Column(Modifier.padding(16.dp)) {
                            Text("${f.changed} saved, ${f.failed} not:", color = Palette.danger)
                            f.errors.forEach { Text(it, color = Palette.danger, style = MaterialTheme.typography.bodySmall) }
                        }
                    }
                }
            }

            problem?.let { Text(it, Modifier.fillMaxWidth().padding(16.dp, 4.dp), color = Palette.danger, style = MaterialTheme.typography.bodyMedium) }
            val shown = if (line.isBlank()) emptyList() else choices
            if (line.isBlank() || shown.isNotEmpty()) {
                LazyRow(Modifier.fillMaxWidth(), contentPadding = PaddingValues(16.dp, 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (line.isBlank()) {
                        items(starters.size) { i -> Pill(starters[i]) { fill(starters[i]) } }
                    } else {
                        items(shown.size) { i ->
                            val c = shown[i]
                            Pill(c.label + if (c.sub.isNotEmpty()) " · ${c.sub}" else "") {
                                editor.choose(i.toUInt())
                                if (c.lineNext != c.line) fill(c.lineNext)
                                else if (c.line.endsWith(":") || c.line.isEmpty()) fill(c.line)
                                else { line = c.line; run(c.line) }
                            }
                        }
                    }
                }
            }
            Row(Modifier.fillMaxWidth().padding(16.dp, 4.dp, 16.dp, 8.dp), verticalAlignment = Alignment.CenterVertically) {
                LineField(line, { line = it }, "title:… artist:… date:…", Modifier.weight(1f).focusRequester(focus), onDone = { if (line.isNotBlank()) run(line) })
                TextButton({ run(line) }, enabled = line.isNotBlank()) { Text("Add") }
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

@Composable
private fun Pill(text: String, click: () -> Unit) {
    Text(
        text, maxLines = 1, style = MaterialTheme.typography.labelLarge, color = Palette.accent,
        modifier = Modifier.clip(RoundedCornerShape(50)).border(1.dp, Palette.line, RoundedCornerShape(50)).clickable { click() }.padding(12.dp, 7.dp),
    )
}

/** One field: its name, its value now (and what it was), its flags, and ✕ where it can come off. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun RowItem(row: EditorRow, run: (String) -> Unit, fill: (String) -> Unit) {
    val start = lineFor(row)
    val remove = removeFor(row)
    val adding = row.kind.startsWith("add_")
    Column(Modifier.fillMaxWidth().clickable(enabled = start != null) { fill(start!!) }.padding(16.dp, 6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(row.key, color = Palette.muted, style = MaterialTheme.typography.labelSmall)
                Text(
                    if (adding) "add…" else row.value.ifEmpty { "none" },
                    color = if (adding || row.kind == "removed") Palette.faint else Palette.text,
                    textDecoration = if (row.kind == "removed") TextDecoration.LineThrough else null,
                    style = MaterialTheme.typography.bodyLarge,
                )
                row.was?.let { Text(if (it == "new") "new" else "was: $it", color = Palette.accent, style = MaterialTheme.typography.labelSmall) }
            }
            if (remove != null) TextButton({ run(remove) }) { Text("✕", color = Palette.muted) }
        }
        if (row.flags.isNotEmpty()) FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            row.flags.forEach { FlagTag(it, fill) }
        }
    }
}

@Composable
private fun FlagTag(flag: EditorFlag, fill: (String) -> Unit) {
    val color: Color = when (flag.state) {
        "fixed" -> Palette.accent
        "new" -> Palette.danger
        "still" -> Palette.muted
        else -> Palette.muted
    }
    val word = when (flag.state) {
        "fixed" -> "fixed: "
        "new" -> "new: "
        "still" -> "still: "
        else -> ""
    }
    Text(
        "⚑ $word${flag.label.lowercase()}" + if (flag.fix != null && flag.state != "fixed") "  · fix" else "",
        color = color, style = MaterialTheme.typography.labelMedium,
        modifier = Modifier.clickable(enabled = flag.fix != null && flag.state != "fixed") { fill(flag.fix!!) }.padding(vertical = 2.dp),
    )
}

/** What the changes do, in the history's words, and what the server's checks say. */
@Composable
private fun Preview(state: EditorState) {
    if (state.chips.isEmpty() && state.tracks.isEmpty()) return
    Column(Modifier.fillMaxWidth().padding(16.dp, 0.dp)) {
        SectionHeader("What it does")
        state.tracks.forEach { t ->
            t.lines.forEach { Text(it, color = Palette.muted, style = MaterialTheme.typography.bodyMedium) }
            t.warnings.forEach { Text("⚠ $it", style = MaterialTheme.typography.bodyMedium) }
            t.error?.let { Text(it, color = Palette.danger, style = MaterialTheme.typography.bodyMedium) }
            if (t.fixed.isNotEmpty()) Text("fixes: ${t.fixed.joinToString(", ")}", color = Palette.accent, style = MaterialTheme.typography.labelMedium)
            if (t.newFlags.isNotEmpty()) Text("new: ${t.newFlags.joinToString(", ")}", color = Palette.danger, style = MaterialTheme.typography.labelMedium)
            if (t.still.isNotEmpty()) Text("still: ${t.still.joinToString(", ")}", color = Palette.faint, style = MaterialTheme.typography.labelMedium)
        }
        if (state.makes.isNotEmpty()) Text("Makes: ${state.makes.joinToString(", ")}", color = Palette.muted, style = MaterialTheme.typography.labelMedium)
        state.blocked?.let { Text(it, color = Palette.faint) }
        state.previewError?.let { Text("The server couldn't check it: $it", color = Palette.danger) }
        Spacer(Modifier.height(4.dp))
    }
}
