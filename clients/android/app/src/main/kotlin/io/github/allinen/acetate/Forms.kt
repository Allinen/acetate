package io.github.allinen.acetate

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import uniffi.acetate_ffi.AlbumForm
import uniffi.acetate_ffi.ArtistForm
import uniffi.acetate_ffi.CoreException
import uniffi.acetate_ffi.EditAlias
import uniffi.acetate_ffi.EditCredit

/** Which form is open over everything. */
sealed interface FormRef {
    data class Covers(val album: String) : FormRef
    data class Album(val id: String) : FormRef
    data class Artist(val id: String) : FormRef
}

/** What a form screen is: Cancel, the title, Save, and the fields below (scrolling, above the keyboard). */
@Composable
private fun FormFrame(
    title: String, canSave: Boolean, saving: Boolean, dirty: Boolean, close: () -> Unit, save: () -> Unit,
    content: @Composable () -> Unit,
) {
    var leaving by remember { mutableStateOf(false) }
    BackHandler { if (dirty && !saving) leaving = true else close() }
    Surface(Modifier.fillMaxSize(), color = Palette.bg) {
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal)).imePadding()) {
            Row(Modifier.fillMaxWidth().padding(8.dp, 4.dp), verticalAlignment = Alignment.CenterVertically) {
                TextButton({ if (dirty && !saving) leaving = true else close() }) { Text("Cancel") }
                Text(title, Modifier.weight(1f).padding(horizontal = 4.dp), maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMedium)
                Button(save, enabled = canSave && !saving) { Text(if (saving) "Saving…" else "Save") }
            }
            Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(16.dp, 0.dp, 16.dp, 24.dp)) { content() }
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
private fun Pill(text: String, selected: Boolean = false, click: () -> Unit) {
    Text(
        text, maxLines = 1, style = MaterialTheme.typography.labelLarge, color = if (selected) Palette.bg else Palette.accent,
        modifier = Modifier.clip(RoundedCornerShape(50)).then(if (selected) Modifier.background(Palette.accent) else Modifier.border(1.dp, Palette.line, RoundedCornerShape(50)))
            .clickable { click() }.padding(12.dp, 7.dp),
    )
}

/** What to say about a save that failed: someone else saved the same thing in between. */
@Composable
private fun SaveProblem(problem: String?, stale: Boolean, reload: () -> Unit) {
    if (problem == null) return
    Column(Modifier.padding(top = 12.dp)) {
        Text(problem, color = Palette.danger)
        if (stale) TextButton(reload) { Text("Load their version (your changes here are dropped)") }
    }
}

/** An album: title, kind, date and its artists, saved as one revision. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun AlbumFormScreen(model: Model, id: String) {
    var loaded by remember(id) { mutableStateOf<AlbumForm?>(null) }
    var loadError by remember(id) { mutableStateOf<String?>(null) }
    var attempt by remember(id) { mutableStateOf(0) }
    LaunchedEffect(id, attempt) {
        loadError = null
        try { loaded = model.core.albumForm(id) } catch (e: CoreException.Failed) { loadError = e.reason }
    }
    val form = loaded
    if (form == null) {
        FormFrame("Edit album", false, false, false, { model.form = null }, {}) {
            Text(loadError ?: "Loading…", Modifier.padding(top = 16.dp), color = if (loadError != null) Palette.danger else Palette.muted)
        }
        return
    }
    var title by remember(form) { mutableStateOf(form.title) }
    var kind by remember(form) { mutableStateOf(form.kind) }
    var date by remember(form) { mutableStateOf(form.releaseDate) }
    val credits = remember(form) { mutableStateListOf<EditCredit>().apply { addAll(form.credits) } }
    var reason by remember(form) { mutableStateOf("") }
    var saving by remember(form) { mutableStateOf(false) }
    var problem by remember(form) { mutableStateOf<String?>(null) }
    var stale by remember(form) { mutableStateOf(false) }
    var artistText by remember(form) { mutableStateOf("") }
    val scope = rememberCoroutineScope()
    val dirty = title != form.title || kind != form.kind || date != form.releaseDate || credits.toList() != form.credits

    FormFrame(
        "Edit album", title.isNotBlank() && dirty, saving, dirty, { model.form = null },
        save = {
            saving = true
            problem = null
            scope.launch {
                try {
                    model.core.saveAlbum(form.copy(title = title, kind = kind, releaseDate = date, credits = credits.toList()), reason)
                    model.core.startSync()
                    model.notice = "Saved."
                    model.form = null
                } catch (e: CoreException.Failed) {
                    problem = e.reason
                    stale = e.code == "stale"
                }
                saving = false
            }
        },
    ) {
        OutlinedTextField(title, { title = it }, Modifier.fillMaxWidth().padding(top = 8.dp), label = { Text("Title") }, singleLine = true)
        Text("Kind", Modifier.padding(top = 16.dp, bottom = 4.dp), color = Palette.muted, style = MaterialTheme.typography.labelLarge)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            (form.kinds + listOf(kind)).distinct().forEach { k -> Pill(k, k == kind) { kind = k } }
        }
        OutlinedTextField(kind, { kind = it }, Modifier.fillMaxWidth().padding(top = 8.dp), label = { Text("…or your own word") }, singleLine = true)
        OutlinedTextField(date, { date = it }, Modifier.fillMaxWidth().padding(top = 8.dp), label = { Text("Release date (as written: 2019, 2019-05-12…)") }, singleLine = true)

        Text("Artists", Modifier.padding(top = 20.dp, bottom = 4.dp), color = Palette.accent, style = MaterialTheme.typography.labelLarge)
        credits.forEachIndexed { i, c ->
            var roles by remember { mutableStateOf(false) }
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(c.name + if (c.artistId == null) " (new)" else "", maxLines = 1, overflow = TextOverflow.Ellipsis)
                    c.alias?.let { Text("as “$it”", style = MaterialTheme.typography.bodySmall, color = Palette.muted) }
                }
                Column {
                    TextButton({ roles = true }) { Text(c.role) }
                    DropdownMenu(roles, { roles = false }) {
                        form.roles.forEach { r -> DropdownMenuItem({ Text(r) }, { credits[i] = c.copy(role = r); roles = false }) }
                    }
                }
                TextButton({ if (i > 0) { credits.add(i - 1, credits.removeAt(i)) } }, enabled = i > 0) { Text("▲") }
                TextButton({ if (i < credits.size - 1) { credits.add(i + 1, credits.removeAt(i)) } }, enabled = i < credits.size - 1) { Text("▼") }
                TextButton({ credits.removeAt(i) }) { Text("✕", color = Palette.muted) }
            }
        }
        // Add an artist: those the library has by that name, or a new one (made when this is saved).
        OutlinedTextField(artistText, { artistText = it }, Modifier.fillMaxWidth().padding(top = 8.dp), label = { Text("Add an artist") }, singleLine = true)
        if (artistText.isNotBlank()) {
            var found by remember { mutableStateOf<List<uniffi.acetate_ffi.ArtistRow>>(emptyList()) }
            LaunchedEffect(artistText, model.catalogVersion) { found = model.core.search(artistText, 6uL).artists }
            val add = { id: String?, name: String ->
                if (id == null || credits.none { it.artistId == id }) {
                    val role = if (credits.none { it.role == "primary" }) "primary" else "featured"
                    credits.add(EditCredit(id, name, role, null, null))
                }
                artistText = ""
            }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp), modifier = Modifier.padding(top = 6.dp)) {
                found.forEach { a -> Pill(a.name) { add(a.id, a.name) } }
                Pill("New artist “${artistText.trim()}”") { add(null, artistText.trim()) }
            }
        }

        OutlinedTextField(reason, { reason = it }, Modifier.fillMaxWidth().padding(top = 20.dp), label = { Text("Why (optional, kept in the history)") }, singleLine = true)
        SaveProblem(problem, stale) { loaded = null; attempt++ }
    }
}

/** An artist: name, what tells it apart from another of the name, and its other names. */
@Composable
fun ArtistFormScreen(model: Model, id: String) {
    var loaded by remember(id) { mutableStateOf<ArtistForm?>(null) }
    var loadError by remember(id) { mutableStateOf<String?>(null) }
    var attempt by remember(id) { mutableStateOf(0) }
    LaunchedEffect(id, attempt) {
        loadError = null
        try { loaded = model.core.artistForm(id) } catch (e: CoreException.Failed) { loadError = e.reason }
    }
    val form = loaded
    if (form == null) {
        FormFrame("Edit artist", false, false, false, { model.form = null }, {}) {
            Text(loadError ?: "Loading…", Modifier.padding(top = 16.dp), color = if (loadError != null) Palette.danger else Palette.muted)
        }
        return
    }
    var name by remember(form) { mutableStateOf(form.name) }
    var tells by remember(form) { mutableStateOf(form.disambiguation) }
    val aliases = remember(form) { mutableStateListOf<EditAlias>().apply { addAll(form.aliases) } }
    var reason by remember(form) { mutableStateOf("") }
    var saving by remember(form) { mutableStateOf(false) }
    var problem by remember(form) { mutableStateOf<String?>(null) }
    var stale by remember(form) { mutableStateOf(false) }
    // A name that's credited somewhere stays (and says where).
    var credited by remember(form) { mutableStateOf<Pair<String, List<String>>?>(null) }
    val scope = rememberCoroutineScope()
    val dirty = name != form.name || tells != form.disambiguation || aliases.filter { it.id != null || it.name.isNotBlank() } != form.aliases

    FormFrame(
        "Edit artist", name.isNotBlank() && dirty, saving, dirty, { model.form = null },
        save = {
            saving = true
            problem = null
            scope.launch {
                try {
                    model.core.saveArtist(form.copy(name = name, disambiguation = tells, aliases = aliases.filter { it.name.isNotBlank() }), reason)
                    model.core.startSync()
                    model.notice = "Saved."
                    model.form = null
                } catch (e: CoreException.Failed) {
                    problem = e.reason
                    stale = e.code == "stale"
                }
                saving = false
            }
        },
    ) {
        OutlinedTextField(name, { name = it }, Modifier.fillMaxWidth().padding(top = 8.dp), label = { Text("Name") }, singleLine = true)
        OutlinedTextField(tells, { tells = it }, Modifier.fillMaxWidth().padding(top = 8.dp), label = { Text("Tells apart from others of the name (optional)") }, singleLine = true)
        Text("Other names", Modifier.padding(top = 20.dp, bottom = 4.dp), color = Palette.accent, style = MaterialTheme.typography.labelLarge)
        aliases.forEachIndexed { i, a ->
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                // A name just added has the cursor.
                val focus = remember { androidx.compose.ui.focus.FocusRequester() }
                if (a.id == null && a.name.isEmpty()) LaunchedEffect(Unit) { try { focus.requestFocus() } catch (_: IllegalStateException) {} }
                OutlinedTextField(a.name, { aliases[i] = a.copy(name = it) }, Modifier.weight(1f).focusRequester(focus), singleLine = true)
                TextButton({
                    val aliasId = a.id
                    if (aliasId == null) aliases.removeAt(i)
                    else scope.launch {
                        try {
                            val uses = model.core.aliasUses(form.id, aliasId)
                            if (uses.isEmpty()) aliases.removeAll { it.id == aliasId } else credited = a.name to uses
                        } catch (e: CoreException.Failed) { problem = e.reason }
                    }
                }) { Text("✕", color = Palette.muted) }
            }
        }
        TextButton({ aliases.add(EditAlias(null, "")) }) { Text("Add a name") }
        OutlinedTextField(reason, { reason = it }, Modifier.fillMaxWidth().padding(top = 12.dp), label = { Text("Why (optional, kept in the history)") }, singleLine = true)
        SaveProblem(problem, stale) { loaded = null; attempt++ }
    }
    credited?.let { (alias, uses) ->
        AlertDialog(
            onDismissRequest = { credited = null },
            title = { Text("“$alias” is still credited") },
            text = { Column { Text("It stays until these are changed:"); uses.take(12).forEach { Text("• $it") }; if (uses.size > 12) Text("…and ${uses.size - 12} more") } },
            confirmButton = { TextButton({ credited = null }) { Text("OK") } },
        )
    }
}
