package io.github.allinen.acetate

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import uniffi.acetate_ffi.MarkOn
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import coil3.network.NetworkHeaders
import coil3.network.httpHeaders
import coil3.request.ImageRequest
import uniffi.acetate_ffi.AlbumRow
import uniffi.acetate_ffi.AlbumView
import uniffi.acetate_ffi.ArtistRow
import uniffi.acetate_ffi.ArtistView
import uniffi.acetate_ffi.ImageSize
import uniffi.acetate_ffi.TrackRow
import uniffi.acetate_ffi.TrackView

private const val PAGE = 64

/** Rows of a view, read a page at a time as the list scrolls to them. */
class Rows<T>(val count: Int, private val read: (start: Int, n: Int) -> List<T>) {
    private val pages = HashMap<Int, List<T>>()

    operator fun get(index: Int): T? =
        pages.getOrPut(index / PAGE) { read(index / PAGE * PAGE, PAGE) }.getOrNull(index % PAGE)
}

@Composable
fun AlbumList(model: Model, view: AlbumView) {
    val rows = remember(view) { Rows(view.count().toInt()) { s, n -> view.rows(s.toULong(), n.toULong()) } }
    LazyColumn(Modifier.fillMaxWidth()) {
        items(rows.count, key = { it }) { i -> rows[i]?.let { AlbumItem(model, it) } }
    }
}

@Composable
fun ArtistList(model: Model, view: ArtistView) {
    val rows = remember(view) { Rows(view.count().toInt()) { s, n -> view.rows(s.toULong(), n.toULong()) } }
    LazyColumn(Modifier.fillMaxWidth()) {
        items(rows.count, key = { it }) { i -> rows[i]?.let { ArtistItem(model, it) } }
    }
}

@Composable
fun TrackList(model: Model, view: TrackView) {
    val rows = remember(view) { Rows(view.count().toInt()) { s, n -> view.rows(s.toULong(), n.toULong()) } }
    LazyColumn(Modifier.fillMaxWidth()) {
        items(rows.count, key = { it }) { i ->
            rows[i]?.let { row -> TrackItem(model, row) { model.playback.playView(view, i.toULong()) } }
        }
    }
}

@Composable
fun AlbumItem(model: Model, row: AlbumRow) {
    Item(
        model, row.cover, row.title,
        listOfNotNull(row.artist, row.year?.toString()).joinToString(" · "),
        Modifier.clickable { model.pages.add(Page.Album(row.id)) },
    )
}

@Composable
fun ArtistItem(model: Model, row: ArtistRow) {
    Item(
        model, null, row.name, "${row.albumCount} albums · ${row.trackCount} tracks",
        Modifier.clickable { model.pages.add(Page.Artist(row.id)) },
    )
}

/** What a long press on a track offers; `extra` adds items of the page's own (given `close`). */
@Composable
fun TrackMenu(
    model: Model, id: String, starred: Boolean, open: Boolean, close: () -> Unit,
    extra: @Composable (() -> Unit) -> Unit = {},
) {
    var picking by remember { mutableStateOf(false) }
    DropdownMenu(open, close) {
        DropdownMenuItem({ Text("Play next") }, { model.playback.enqueueTracks(listOf(id), true); close() })
        DropdownMenuItem({ Text("Add to queue") }, { model.playback.enqueueTracks(listOf(id), false); close() })
        DropdownMenuItem({ Text("Add to playlist…") }, { picking = true; close() })
        DropdownMenuItem({ Text(if (starred) "Unstar" else "Star") }, { model.core.setStarred(MarkOn.TRACK, id, !starred); close() })
        extra(close)
    }
    if (picking) PlaylistPicker(model, listOf(id)) { picking = false }
}

/** A track: tap plays, long press opens a menu (play next, add to queue, star…). */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun TrackItem(model: Model, row: TrackRow, extra: @Composable (() -> Unit) -> Unit = {}, onClick: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    val starred = rememberMark(model, row.id).starred
    Box {
        Item(
            model, row.cover, (if (starred) "★ " else "") + row.title,
            listOfNotNull(row.artist, row.album).joinToString(" · "),
            Modifier.combinedClickable(onClick = onClick, onLongClick = { menu = true }),
            trailing = row.durationMs?.let(::duration), highlight = model.player.current?.id == row.id,
        )
        TrackMenu(model, row.id, starred, menu, { menu = false }, extra)
    }
}

@Composable
private fun Item(
    model: Model, cover: String?, title: String, subtitle: String, modifier: Modifier,
    trailing: String? = null, highlight: Boolean = false,
) {
    Row(modifier.fillMaxWidth().padding(16.dp, 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Cover(model, cover, 56)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(
                title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge,
                color = if (highlight) MaterialTheme.colorScheme.primary else Color.Unspecified,
            )
            Text(
                subtitle, maxLines = 1, overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        trailing?.let {
            Spacer(Modifier.width(12.dp))
            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

fun duration(ms: Long): String {
    val s = ms / 1000
    return "%d:%02d".format(s / 60, s % 60)
}

@Composable
fun Cover(model: Model, key: String?, size: Int, modifier: Modifier = Modifier) {
    val shaped = modifier.size(size.dp).clip(RoundedCornerShape(4.dp))
    val request = key?.let { model.core.imageRequest(it, if (size > 100) ImageSize.STANDARD else ImageSize.SMALL) }
    if (request == null) {
        Box(shaped.background(MaterialTheme.colorScheme.surfaceVariant))
        return
    }
    AsyncImage(
        model = ImageRequest.Builder(LocalContext.current)
            .data(request.url)
            .httpHeaders(NetworkHeaders.Builder().set("Authorization", request.authorization).build())
            .build(),
        contentDescription = null, modifier = shaped, contentScale = ContentScale.Crop,
    )
}
