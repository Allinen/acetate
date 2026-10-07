package io.github.allinen.acetate

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
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
import uniffi.acetate_ffi.ImageSize

private const val PAGE = 64

/** Rows of a view, read a page at a time as the list scrolls to them. */
private class Rows<T>(val count: Int, private val read: (start: Int, n: Int) -> List<T>) {
    private val pages = HashMap<Int, List<T>>()

    operator fun get(index: Int): T? =
        pages.getOrPut(index / PAGE) { read(index / PAGE * PAGE, PAGE) }.getOrNull(index % PAGE)
}

@Composable
fun AlbumList(model: Model, view: AlbumView) {
    val rows = remember(view) { Rows(view.count().toInt()) { start, n -> view.rows(start.toULong(), n.toULong()) } }
    LazyColumn(state = rememberLazyListState(), modifier = Modifier.fillMaxWidth()) {
        items(rows.count, key = { it }) { i -> rows[i]?.let { AlbumItem(model, it) } }
    }
}

@Composable
private fun AlbumItem(model: Model, row: AlbumRow) {
    Row(Modifier.fillMaxWidth().padding(16.dp, 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Cover(model, row.cover, 56)
        Spacer(Modifier.width(12.dp))
        Column {
            Text(row.title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge)
            Text(
                listOfNotNull(row.artist, row.year?.toString()).joinToString(" · "),
                maxLines = 1, overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
fun Cover(model: Model, key: String?, size: Int) {
    val shape = RoundedCornerShape(4.dp)
    val modifier = Modifier.size(size.dp).clip(shape)
    val request = key?.let { model.core.imageRequest(it, ImageSize.SMALL) }
    if (request == null) {
        Box(modifier.background(MaterialTheme.colorScheme.surfaceVariant))
        return
    }
    AsyncImage(
        model = ImageRequest.Builder(LocalContext.current)
            .data(request.url)
            .httpHeaders(NetworkHeaders.Builder().set("Authorization", request.authorization).build())
            .build(),
        contentDescription = null, modifier = modifier, contentScale = ContentScale.Crop,
    )
}
