package io.github.allinen.acetate

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.acetate_ffi.CoreException
import uniffi.acetate_ffi.LyricsEntry
import uniffi.acetate_ffi.SyncLine

/** `m:ss.t` for a time in milliseconds. */
private fun clockTenths(ms: Long): String {
    val m = ms.coerceAtLeast(0)
    return "%d:%02d.%d".format(m / 60000, m / 1000 % 60, m % 1000 / 100)
}

/**
 * A track's lyrics: its entries (a track may have several: languages, official or
 * transcribed), one open at a time with its text, whether it's official and its language;
 * saved as one revision. "Sync…" times the lines while the song plays.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun LyricsFormScreen(model: Model, track: String) {
    var entries by remember(track) { mutableStateOf<List<LyricsEntry>?>(null) }
    var loadError by remember(track) { mutableStateOf<String?>(null) }
    var attempt by remember(track) { mutableIntStateOf(0) }
    // The entry open: its place in the list, or -1 for a new one.
    var open by remember(track) { mutableIntStateOf(0) }
    LaunchedEffect(track, attempt) {
        loadError = null
        try { entries = model.core.lyricsEntries(track) } catch (e: CoreException.Failed) { loadError = e.reason }
    }
    val list = entries
    if (list == null) {
        FormFrame("Lyrics", false, false, false, { model.form = null }, {}) {
            Text(loadError ?: "Loading…", Modifier.padding(top = 16.dp), color = if (loadError != null) Palette.danger else Palette.muted)
        }
        return
    }
    val entry = list.getOrNull(open)
    var body by remember(track, entry?.id, attempt) { mutableStateOf(entry?.body ?: "") }
    var official by remember(track, entry?.id, attempt) { mutableStateOf(entry?.official ?: false) }
    var language by remember(track, entry?.id, attempt) { mutableStateOf(entry?.language ?: "") }
    var reason by remember(track, entry?.id, attempt) { mutableStateOf("") }
    var saving by remember(track) { mutableStateOf(false) }
    var problem by remember(track) { mutableStateOf<String?>(null) }
    var stale by remember(track) { mutableStateOf(false) }
    var syncing by remember(track) { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val dirty = body != (entry?.body ?: "") || official != (entry?.official ?: false) || language != (entry?.language ?: "")

    FormFrame(
        "Lyrics", body.isNotBlank() && dirty, saving, dirty, { model.form = null },
        save = {
            saving = true
            problem = null
            scope.launch {
                try {
                    model.core.saveLyrics(track, entry, body, official, language, reason)
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
        FlowRow(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            list.forEachIndexed { i, e -> FormPill(e.name, i == open) { if (!dirty) open = i else problem = "Save or discard the changes to this entry first." } }
            FormPill("+ New entry", open == -1) { if (!dirty) open = -1 else problem = "Save or discard the changes to this entry first." }
        }
        Row(Modifier.fillMaxWidth().padding(top = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Official (as the artist published them)", Modifier.weight(1f))
            Switch(official, { official = it })
        }
        OutlinedTextField(language, { language = it }, Modifier.fillMaxWidth().padding(top = 8.dp), label = { Text("Language (optional)") }, singleLine = true)
        OutlinedTextField(
            body, { body = it }, Modifier.fillMaxWidth().padding(top = 8.dp).heightIn(min = 280.dp),
            label = { Text("The lyrics (plain, or LRC with [mm:ss.xx] times)") },
        )
        OutlinedButton({ syncing = true }, Modifier.padding(top = 8.dp), enabled = body.isNotBlank()) { Text("Sync… (time the lines while it plays)") }
        OutlinedTextField(reason, { reason = it }, Modifier.fillMaxWidth().padding(top = 12.dp), label = { Text("Why (optional, kept in the history)") }, singleLine = true)
        problem?.let {
            Text(it, Modifier.padding(top = 12.dp), color = Palette.danger)
            if (stale) TextButton({ entries = null; attempt++; problem = null }) { Text("Load their version (your changes here are dropped)") }
        }
    }
    if (syncing) SyncScreen(model, track, body) { text -> syncing = false; if (text != null) body = text }
}

/**
 * Timing lyrics while listening: the text as lines, each with its time. The track plays;
 * a tap on the big button gives the line at the cursor the time playing now (as heard:
 * the Bluetooth setting counts) and moves to the next, so tapping along as it's sung times
 * the whole song. A line's menu: start here, play from just before it, 0.1 s earlier or
 * later, no time, its words, a line after it, a heading before it, remove it.
 */
@Composable
private fun SyncScreen(model: Model, track: String, body: String, done: (String?) -> Unit) {
    val draft = remember(track) { model.core.lyricsDraft(body) }
    var version by remember(track) { mutableIntStateOf(0) }
    val lines = remember(version) { draft.lines() }
    val cursor = remember(version) { draft.cursor().toInt() }
    var position by remember(track) { mutableLongStateOf(0L) }
    var offset by remember(track) { mutableIntStateOf(LyricsTiming.offsetMs()) }
    var menu by remember(track) { mutableStateOf<Int?>(null) }
    var editing by remember(track) { mutableStateOf<Int?>(null) }
    var leaving by remember(track) { mutableStateOf(false) }
    val list = rememberLazyListState()
    val state = model.player

    // The song plays (this track), and its position is read often.
    LaunchedEffect(track) {
        if (model.player.current?.id != track) model.playback.playTracks(listOf(track), 0uL)
        var asked = 0
        while (true) {
            if (asked++ % 10 == 0) offset = LyricsTiming.offsetMs()
            position = model.playback.state().positionMs.toLong()
            delay(100)
        }
    }
    LaunchedEffect(cursor) { list.animateScrollToItem((cursor - 2).coerceAtLeast(0)) }
    val heard = position + offset
    // The line being sung: the last timed one that has started.
    val now = lines.indexOfLast { (it.showsMs ?: Long.MAX_VALUE) <= heard }
    fun changed() { version++ }

    BackHandler { if (draft.changed()) leaving = true else done(null) }
    Surface(Modifier.fillMaxSize(), color = Palette.bg) {
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Bottom + WindowInsetsSides.Horizontal))) {
            Row(Modifier.fillMaxWidth().padding(8.dp, 4.dp), verticalAlignment = Alignment.CenterVertically) {
                TextButton({ if (draft.changed()) leaving = true else done(null) }) { Text("Cancel") }
                Text("Sync · ${clockTenths(position)}", Modifier.weight(1f).padding(horizontal = 4.dp), style = MaterialTheme.typography.titleMedium)
                TextButton({ if (draft.undo()) changed() }) { Text("Undo") }
                Button({ done(draft.text()) }, enabled = draft.changed()) { Text("Done") }
            }
            LazyColumn(Modifier.weight(1f).fillMaxWidth(), state = list) {
                items(lines.size, key = { it }) { i ->
                    val line: SyncLine = lines[i]
                    Box {
                        Row(
                            Modifier.fillMaxWidth()
                                .background(if (i == cursor) Palette.accentSoft else if (i == now) Palette.surfaceHi else Palette.bg)
                                .clickable { menu = i }.padding(16.dp, 8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(
                                if (line.tag || line.heading) "" else line.showsMs?.let { clockTenths(it.toLong()) } ?: "—",
                                Modifier.width(64.dp), color = if (line.showsMs == null) Palette.faint else Palette.muted, style = MaterialTheme.typography.labelMedium,
                            )
                            Column(Modifier.weight(1f)) {
                                Text(
                                    if (line.heading) "[${line.text}]" else line.text.ifEmpty { "♪" },
                                    maxLines = 2, overflow = TextOverflow.Ellipsis,
                                    color = if (line.heading) Palette.accent else if (line.tag) Palette.faint else Palette.text,
                                )
                                line.language?.let { Text(it, color = Palette.faint, style = MaterialTheme.typography.labelSmall) }
                            }
                        }
                        DropdownMenu(menu == i, { menu = null }) {
                            if (!line.tag && !line.heading) {
                                DropdownMenuItem({ Text("Start here (tapping times this line next)") }, { draft.setCursor(i.toUInt()); changed(); menu = null })
                                if (line.showsMs != null) DropdownMenuItem({ Text("Play from just before it") }, { model.playback.seek(((line.showsMs ?: 0L) - 2000L).coerceAtLeast(0L).toULong()); menu = null })
                                DropdownMenuItem({ Text("0.1 s earlier") }, { draft.nudge(i.toUInt(), -100L, heard); changed() })
                                DropdownMenuItem({ Text("0.1 s later") }, { draft.nudge(i.toUInt(), 100L, heard); changed() })
                                if (line.showsMs != null) DropdownMenuItem({ Text("No time") }, { draft.clearTime(i.toUInt()); changed(); menu = null })
                            }
                            if (!line.tag) DropdownMenuItem({ Text(if (line.heading) "Edit the heading" else "Edit the words") }, { editing = i; menu = null })
                            DropdownMenuItem({ Text("Add a line after it") }, { draft.insertAfter(i.toUInt()); changed(); menu = null; editing = i + 1 })
                            DropdownMenuItem({ Text("Add a heading before it") }, { draft.insertHeadingBefore(i.toUInt()); changed(); menu = null; editing = i })
                            DropdownMenuItem({ Text("Remove it") }, { draft.remove(i.toUInt()); changed(); menu = null })
                        }
                    }
                }
            }
            Row(Modifier.fillMaxWidth().padding(8.dp), horizontalArrangement = Arrangement.SpaceEvenly, verticalAlignment = Alignment.CenterVertically) {
                TextButton({ model.playback.seek((position - 5000).coerceAtLeast(0).toULong()) }) { Text("−5 s") }
                TextButton({ model.playback.setPaused(isPlaying(state)) }) { Text(if (isPlaying(state)) "Pause" else "Play") }
                TextButton({ model.playback.seek((position + 5000).toULong()) }) { Text("+5 s") }
            }
            Button(
                { if (draft.tap(heard) != null) changed() },
                Modifier.fillMaxWidth().padding(16.dp, 0.dp, 16.dp, 12.dp).height(72.dp),
                enabled = lines.getOrNull(cursor)?.let { !it.tag && !it.heading } == true,
            ) {
                Text(lines.getOrNull(cursor)?.text?.take(40)?.let { "Tap: “$it” starts now" } ?: "Nothing to time", maxLines = 1)
            }
        }
    }
    editing?.let { n ->
        var text by remember(n) { mutableStateOf(lines.getOrNull(n)?.text ?: "") }
        AlertDialog(
            onDismissRequest = { editing = null },
            title = { Text(if (lines.getOrNull(n)?.heading == true) "Heading" else "The words") },
            text = { FocusedField(text, { text = it }, Modifier.fillMaxWidth()) },
            confirmButton = { TextButton({ draft.setText(n.toUInt(), text); changed(); editing = null }) { Text("OK") } },
            dismissButton = { TextButton({ editing = null }) { Text("Cancel") } },
        )
    }
    if (leaving) AlertDialog(
        onDismissRequest = { leaving = false },
        title = { Text("Throw away the timing?") },
        text = { Text("The lyrics aren't changed until you press Done here and then Save.") },
        confirmButton = { TextButton({ leaving = false; done(null) }) { Text("Throw away") } },
        dismissButton = { TextButton({ leaving = false }) { Text("Keep going") } },
    )
}
