package io.github.allinen.acetate

import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.runtime.mutableIntStateOf
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.FilterChip
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import uniffi.acetate_ffi.MarkOn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import uniffi.acetate_ffi.QueuePart
import uniffi.acetate_ffi.TrackRow

/** Where playback is now, read every 250 ms while something plays (the core moves it on smoothly). */
@Composable
fun rememberPosition(model: Model): Long {
    var position by remember { mutableLongStateOf(model.player.positionMs.toLong()) }
    val state = model.player
    LaunchedEffect(state.current?.id, state.playing, state.paused) {
        position = model.playback.state().positionMs.toLong()
        while (state.playing && !state.paused) {
            delay(250)
            position = model.playback.state().positionMs.toLong()
        }
    }
    return position
}

private fun isPlaying(s: uniffi.acetate_ffi.PlayerState) = s.playing && !s.paused

// ---- Icons (a few shapes: no icon library needed) ----

@Composable
private fun PlayPauseIcon(playing: Boolean, color: Color, size: Int = 28) {
    Canvas(Modifier.size(size.dp)) {
        val w = this.size.width
        val h = this.size.height
        if (playing) {
            drawRect(color, Offset(w * 0.2f, h * 0.15f), androidx.compose.ui.geometry.Size(w * 0.2f, h * 0.7f))
            drawRect(color, Offset(w * 0.6f, h * 0.15f), androidx.compose.ui.geometry.Size(w * 0.2f, h * 0.7f))
        } else {
            drawPath(Path().apply {
                moveTo(w * 0.25f, h * 0.12f)
                lineTo(w * 0.85f, h * 0.5f)
                lineTo(w * 0.25f, h * 0.88f)
                close()
            }, color)
        }
    }
}

@Composable
private fun SkipIcon(forward: Boolean, color: Color, size: Int = 28) {
    Canvas(Modifier.size(size.dp)) {
        val w = this.size.width
        val h = this.size.height
        val dir = if (forward) 1f else -1f
        val mid = w / 2
        // A triangle and a bar at its tip.
        drawPath(Path().apply {
            moveTo(mid - dir * w * 0.3f, h * 0.2f)
            lineTo(mid + dir * w * 0.2f, h * 0.5f)
            lineTo(mid - dir * w * 0.3f, h * 0.8f)
            close()
        }, color)
        drawRect(
            color,
            Offset(if (forward) mid + w * 0.2f else mid - w * 0.3f, h * 0.2f),
            androidx.compose.ui.geometry.Size(w * 0.1f, h * 0.6f),
        )
    }
}

@Composable
private fun RoundButton(onClick: () -> Unit, size: Int, content: @Composable () -> Unit) {
    Box(
        Modifier.size(size.dp).clip(CircleShape).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { content() }
}

/** Shuffle or repeat-one: lit (accent on its soft color) when on. */
@Composable
private fun ModeButton(on: Boolean, onClick: () -> Unit, icon: @Composable (Color) -> Unit) {
    val color = if (on) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant
    Box(
        Modifier.size(44.dp).clip(CircleShape)
            .background(if (on) MaterialTheme.colorScheme.primaryContainer else Color.Transparent)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { icon(color) }
}

/** The desktop's shuffle (two crossing arrows) and repeat-one (a loop with a "1"). */
@Composable
private fun ModeIcon(repeat: Boolean, color: Color) {
    val density = androidx.compose.ui.platform.LocalDensity.current
    Canvas(Modifier.size(24.dp)) {
        val s = size.width / 2.6f
        val c = Offset(size.width / 2, size.height / 2)
        val stroke = androidx.compose.ui.graphics.drawscope.Stroke(width = s * 0.2f, cap = androidx.compose.ui.graphics.StrokeCap.Round)
        fun arrowHead(tip: Offset) = drawPath(Path().apply {
            moveTo(tip.x + s * 0.45f, tip.y)
            lineTo(tip.x - s * 0.05f, tip.y - s * 0.35f)
            lineTo(tip.x - s * 0.05f, tip.y + s * 0.35f)
            close()
        }, color)
        if (!repeat) {
            for ((from, to) in listOf(-0.6f to 0.6f, 0.6f to -0.6f)) {
                val a = Offset(c.x - s, c.y + from * s)
                val b = Offset(c.x + s * 0.7f, c.y + to * s)
                drawLine(color, a, b, stroke.width, stroke.cap)
                arrowHead(b)
            }
        } else {
            val tl = Offset(c.x - s, c.y - s * 0.65f)
            drawRoundRect(
                color, tl, androidx.compose.ui.geometry.Size(s * 2f, s * 1.3f),
                androidx.compose.ui.geometry.CornerRadius(s * 0.4f), style = stroke,
            )
            arrowHead(Offset(c.x + s * 0.35f, tl.y))
            // The "1": a stem with a flag.
            drawLine(color, Offset(c.x, c.y - s * 0.3f), Offset(c.x, c.y + s * 0.3f), stroke.width, stroke.cap)
            drawLine(color, Offset(c.x - s * 0.2f, c.y - s * 0.15f), Offset(c.x, c.y - s * 0.3f), stroke.width, stroke.cap)
        }
    }
}

// ---- The mini player ----

@Composable
fun MiniPlayer(model: Model) {
    val state = model.player
    val current = state.current ?: return
    val position = rememberPosition(model)
    Surface(color = Palette.surface, modifier = Modifier.fillMaxWidth().clickable { model.nowPlaying = true }) {
        Column {
            val length = current.durationMs ?: 0L
            LinearProgressIndicator(
                progress = { if (length > 0) (position.toFloat() / length).coerceIn(0f, 1f) else 0f },
                modifier = Modifier.fillMaxWidth().height(2.dp),
            )
            Row(Modifier.padding(12.dp, 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Cover(model, current.cover, 44)
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(current.title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium)
                    Text(
                        current.artist, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                val ink = MaterialTheme.colorScheme.onSurface
                RoundButton({ model.playback.setPaused(isPlaying(state)) }, 48) { PlayPauseIcon(isPlaying(state), ink) }
                RoundButton({ model.playback.next() }, 48) { SkipIcon(true, ink) }
            }
        }
    }
}

// ---- Now playing ----

private fun clock(ms: Long): String {
    val s = (ms / 1000).coerceAtLeast(0)
    return "%d:%02d".format(s / 60, s % 60)
}

@Composable
fun NowPlaying(model: Model) {
    BackHandler { model.nowPlaying = false }
    val state = model.player
    val current = state.current
    val position = rememberPosition(model)
    // While the thumb is held, the slider shows where it is, not where playback is.
    var dragging by remember { mutableStateOf<Float?>(null) }
    // Read again when the queue changes.
    val queued = remember(model.playerVersion) { model.playback.queuedRows() }
    val ahead = remember(model.playerVersion) {
        val from = state.listIndex.toLong() + 1
        model.playback.listRows(from.toULong(), 200uL)
    }
    val lyrics = remember(current?.id, model.detailsVersion) { current?.let { model.core.lyrics(it.id) } }
    var showLyrics by remember { mutableStateOf(false) }
    if (showLyrics && lyrics != null && current != null) {
        BackHandler { showLyrics = false }
        LyricsScreen(model, lyrics, current.title, current.artist, position, { showLyrics = false })
        return
    }
    Surface(Modifier.fillMaxSize()) {
        LazyColumn(Modifier.safeDrawingPadding(), horizontalAlignment = Alignment.CenterHorizontally) {
            item {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    TextButton({ model.nowPlaying = false }) { Text("Close") }
                    Row {
                        if (current != null) ExtrasButton(model, MarkOn.TRACK, current.id, current.title)
                        if (lyrics != null) TextButton({ showLyrics = true }) { Text("Lyrics") }
                    }
                }
            }
            if (current != null) item {
                Column(Modifier.padding(24.dp, 0.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Cover(model, current.cover, 280)
                    Spacer(Modifier.height(20.dp))
                    Text(current.title, style = MaterialTheme.typography.titleLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    Text(
                        listOfNotNull(current.artist, current.album).joinToString(" · "), maxLines = 2,
                        overflow = TextOverflow.Ellipsis, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.height(14.dp))
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                        RatingBar(model, MarkOn.TRACK, current.id)
                        StarToggle(model, MarkOn.TRACK, current.id)
                    }
                    val length = (current.durationMs ?: 0L).toFloat()
                    Slider(
                        value = dragging ?: position.toFloat().coerceAtMost(length.coerceAtLeast(1f)),
                        onValueChange = { dragging = it },
                        onValueChangeFinished = {
                            dragging?.let { model.playback.seek(it.toLong().toULong()) }
                            dragging = null
                        },
                        valueRange = 0f..length.coerceAtLeast(1f),
                    )
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text(clock((dragging ?: position.toFloat()).toLong()), style = MaterialTheme.typography.labelMedium)
                        Text(clock(length.toLong()), style = MaterialTheme.typography.labelMedium)
                    }
                    val ink = MaterialTheme.colorScheme.onSurface
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        ModeButton(state.shuffle, { model.playback.toggleShuffle() }) { ModeIcon(false, it) }
                        RoundButton({ model.playback.previous() }, 56) { SkipIcon(false, ink, 32) }
                        Box(Modifier.size(64.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary).clickable {
                            model.playback.setPaused(isPlaying(state))
                        }, contentAlignment = Alignment.Center) {
                            PlayPauseIcon(isPlaying(state), MaterialTheme.colorScheme.onPrimary, 32)
                        }
                        RoundButton({ model.playback.next() }, 56) { SkipIcon(true, ink, 32) }
                        ModeButton(state.repeat, { model.playback.toggleRepeat() }) { ModeIcon(true, it) }
                    }
                    Spacer(Modifier.height(12.dp))
                }
            }
            if (queued.isNotEmpty()) {
                item { SectionTitle("Queue", "Clear") { model.playback.clearQueued() } }
                items(queued.size, key = { "q$it" }) { i -> QueueRow(model, queued[i], QueuePart.QUEUED, i) }
            }
            if (ahead.isNotEmpty()) {
                item { SectionTitle("Up next", null) {} }
                items(ahead.size, key = { "l$it" }) { i -> QueueRow(model, ahead[i], QueuePart.LIST, state.listIndex.toInt() + 1 + i) }
            }
        }
    }
}

@Composable
private fun SectionTitle(title: String, action: String?, onAction: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(16.dp, 8.dp, 8.dp, 0.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(title, Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
        if (action != null) TextButton(onAction) { Text(action) }
    }
}

@Composable
private fun QueueRow(model: Model, row: TrackRow, part: QueuePart, index: Int) {
    Row(
        Modifier.fillMaxWidth().clickable { model.playback.playPlace(part, index.toULong()) }.padding(16.dp, 6.dp, 4.dp, 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Cover(model, row.cover, 40)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(row.title, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                row.artist, maxLines = 1, overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        // Only what's been queued by hand can be moved or taken out here.
        if (part == QueuePart.QUEUED) {
            // Drag the handle: a row's height of travel moves it one place.
            var at by remember { mutableIntStateOf(index) }
            var travel by remember { mutableStateOf(0f) }
            val step = with(androidx.compose.ui.platform.LocalDensity.current) { 56.dp.toPx() }
            val last = remember(model.playerVersion) { model.playback.queuedRows().size - 1 }
            Text(
                "≡", Modifier.padding(12.dp).pointerInput(Unit) {
                    detectDragGestures(onDragStart = { at = index; travel = 0f }) { change, drag ->
                        change.consume()
                        travel += drag.y
                        while (travel > step && at < last) { model.playback.moveQueued(at.toULong(), (at + 1).toULong()); at++; travel -= step }
                        while (travel < -step && at > 0) { model.playback.moveQueued(at.toULong(), (at - 1).toULong()); at--; travel += step }
                    }
                },
                style = MaterialTheme.typography.titleLarge, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (part == QueuePart.QUEUED) TextButton({ model.playback.remove(part, index.toULong()) }) { Text("Remove") }
    }
}

/** The lyrics full screen: the lines (the one sung now lit, following it; tap one to go
 *  there), and the transport below. */
@Composable
private fun LyricsScreen(
    model: Model, lyrics: uniffi.acetate_ffi.LyricsView, title: String, artist: String, position: Long, back: () -> Unit,
) {
    val state = model.player
    val list = androidx.compose.foundation.lazy.rememberLazyListState()
    // The last line that has started.
    val now = if (lyrics.synced) lyrics.lines.indexOfLast { (it.atMs ?: ULong.MAX_VALUE).toLong() <= position } else -1
    LaunchedEffect(now) {
        if (now >= 0 && !list.isScrollInProgress) list.animateScrollToItem(now, -250)
    }
    Surface(Modifier.fillMaxSize()) {
        Column(Modifier.safeDrawingPadding()) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                TextButton(back) { Text("Back") }
                Column(Modifier.weight(1f).padding(end = 16.dp)) {
                    Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMedium)
                    Text(artist, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            LazyColumn(Modifier.weight(1f).fillMaxWidth(), state = list, contentPadding = androidx.compose.foundation.layout.PaddingValues(24.dp, 8.dp, 24.dp, 200.dp)) {
                items(lyrics.lines.size, key = { it }) { i ->
                    val line = lyrics.lines[i]
                    Column(
                        Modifier.fillMaxWidth()
                            .clickable(enabled = line.atMs != null) { line.atMs?.let { model.playback.seek(it) } }
                            .padding(vertical = 6.dp),
                    ) {
                        line.heading?.let {
                            Spacer(Modifier.height(10.dp))
                            Text(it, style = MaterialTheme.typography.labelLarge, color = Palette.accent)
                        }
                        Text(
                            line.text.ifEmpty { "♪" }, style = MaterialTheme.typography.titleLarge,
                            color = when {
                                !lyrics.synced -> MaterialTheme.colorScheme.onSurface
                                i == now -> MaterialTheme.colorScheme.onSurface
                                else -> Palette.faint
                            },
                        )
                    }
                }
            }
            val ink = MaterialTheme.colorScheme.onSurface
            Row(Modifier.fillMaxWidth().padding(bottom = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.Center) {
                RoundButton({ model.playback.previous() }, 52) { SkipIcon(false, ink, 30) }
                Box(Modifier.size(60.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary).clickable {
                    model.playback.setPaused(isPlaying(state))
                }, contentAlignment = Alignment.Center) {
                    PlayPauseIcon(isPlaying(state), MaterialTheme.colorScheme.onPrimary, 30)
                }
                RoundButton({ model.playback.next() }, 52) { SkipIcon(true, ink, 30) }
            }
        }
    }
}
