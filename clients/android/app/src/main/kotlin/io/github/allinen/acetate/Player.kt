package io.github.allinen.acetate

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

// ---- The mini player ----

@Composable
fun MiniPlayer(model: Model) {
    val state = model.player
    val current = state.current ?: return
    val position = rememberPosition(model)
    Surface(tonalElevation = 3.dp, modifier = Modifier.fillMaxWidth().clickable { model.nowPlaying = true }) {
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
                RoundButton({ model.playback.setPaused(isPlaying(state)) }, 48) { PlayPauseIcon(!isPlaying(state), ink) }
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
    Surface(Modifier.fillMaxSize()) {
        LazyColumn(Modifier.safeDrawingPadding(), horizontalAlignment = Alignment.CenterHorizontally) {
            item {
                Row(Modifier.fillMaxWidth()) { TextButton({ model.nowPlaying = false }) { Text("Close") } }
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
                    Row(verticalAlignment = Alignment.CenterVertically) {
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
                        FilterChip(state.shuffle, { model.playback.toggleShuffle() }, { Text("Shuffle") })
                        RoundButton({ model.playback.previous() }, 56) { SkipIcon(false, ink, 32) }
                        Box(Modifier.size(64.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary).clickable {
                            model.playback.setPaused(isPlaying(state))
                        }, contentAlignment = Alignment.Center) {
                            PlayPauseIcon(!isPlaying(state), MaterialTheme.colorScheme.onPrimary, 32)
                        }
                        RoundButton({ model.playback.next() }, 56) { SkipIcon(true, ink, 32) }
                        FilterChip(state.repeat, { model.playback.toggleRepeat() }, { Text("Repeat") })
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
        // Only what's been queued by hand can be taken out here.
        if (part == QueuePart.QUEUED) TextButton({ model.playback.remove(part, index.toULong()) }) { Text("Remove") }
    }
}
