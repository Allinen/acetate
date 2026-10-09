package io.github.allinen.acetate

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch

class ReorderItem(val id: String, val title: String, val sub: String)

/** A list to put in another order: saved once, with the ids in the order they were left in.
 *  `save` returns why it couldn't, or null. */
class ReorderSpec(val title: String, val items: List<ReorderItem>, val save: suspend (List<String>) -> String?)

/**
 * Reorder mode: every row has a handle to drag by (and nothing else on the row, so a long
 * press means nothing here). Nothing is written until Save.
 */
@Composable
fun ReorderScreen(model: Model, spec: ReorderSpec) {
    val order = remember(spec) { mutableStateListOf<ReorderItem>().apply { addAll(spec.items) } }
    val lazy = rememberLazyListState()
    var dragging by remember(spec) { mutableStateOf<String?>(null) }
    var offset by remember(spec) { mutableFloatStateOf(0f) }
    var saving by remember(spec) { mutableStateOf(false) }
    var problem by remember(spec) { mutableStateOf<String?>(null) }
    var leaving by remember(spec) { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val changed = order.map { it.id } != spec.items.map { it.id }

    fun close() { model.reorder = null }

    // While a row is dragged: it swaps with the one its middle is over, keeping under the finger.
    fun shift() {
        val shown = lazy.layoutInfo.visibleItemsInfo
        val me = shown.firstOrNull { it.key == dragging } ?: return
        val center = me.offset + me.size / 2f + offset
        val target = shown.firstOrNull { it.key != dragging && center >= it.offset && center <= it.offset + it.size }
        if (target != null) {
            order.add(target.index, order.removeAt(me.index))
            offset += (me.offset - target.offset).toFloat()
        }
        // Near an edge the list scrolls (as long as the finger keeps moving).
        val end = lazy.layoutInfo.viewportEndOffset
        if (center > end - 120) lazy.dispatchRawDelta(30f) else if (center < 120) lazy.dispatchRawDelta(-30f)
    }

    BackHandler { if (changed && !saving) leaving = true else close() }
    Surface(Modifier.fillMaxSize(), color = Palette.bg) {
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Bottom + WindowInsetsSides.Horizontal))) {
            Row(Modifier.fillMaxWidth().padding(8.dp, 4.dp), verticalAlignment = Alignment.CenterVertically) {
                TextButton({ if (changed && !saving) leaving = true else close() }) { Text("Cancel") }
                Text(spec.title, Modifier.weight(1f).padding(horizontal = 4.dp), maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMedium)
                Button({
                    saving = true
                    scope.launch {
                        val why = spec.save(order.map { it.id })
                        saving = false
                        if (why == null) close() else problem = why
                    }
                }, enabled = changed && !saving) { Text(if (saving) "Saving…" else "Save") }
            }
            Text("Drag a row by its handle.", Modifier.padding(16.dp, 0.dp), color = Palette.faint, style = MaterialTheme.typography.labelSmall)
            problem?.let { Text(it, Modifier.padding(16.dp, 4.dp), color = Palette.danger) }
            LazyColumn(Modifier.weight(1f).fillMaxWidth(), state = lazy) {
                items(order, key = { it.id }) { item ->
                    val mine = dragging == item.id
                    Row(
                        Modifier.fillMaxWidth()
                            .then(if (mine) Modifier else Modifier.animateItem())
                            .graphicsLayer { translationY = if (mine) offset else 0f; shadowElevation = if (mine) 12f else 0f }
                            .then(if (mine) Modifier.background(Palette.surfaceHi) else Modifier)
                            .padding(16.dp, 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column(Modifier.weight(1f)) {
                            Text(item.title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            if (item.sub.isNotEmpty()) Text(item.sub, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        // The handle: the only place a drag starts.
                        Text(
                            "≡", Modifier.width(48.dp).padding(8.dp)
                                .pointerInput(item.id) {
                                    detectDragGestures(
                                        onDragStart = { dragging = item.id; offset = 0f },
                                        onDrag = { change, drag -> change.consume(); offset += drag.y; shift() },
                                        onDragEnd = { dragging = null; offset = 0f },
                                        onDragCancel = { dragging = null; offset = 0f },
                                    )
                                },
                            style = MaterialTheme.typography.headlineSmall, color = Palette.muted,
                        )
                    }
                }
            }
        }
    }
    if (leaving) AlertDialog(
        onDismissRequest = { leaving = false },
        title = { Text("Discard the new order?") },
        text = { Text("Nothing has been saved.") },
        confirmButton = { TextButton({ leaving = false; close() }) { Text("Discard") } },
        dismissButton = { TextButton({ leaving = false }) { Text("Keep editing") } },
    )
}
