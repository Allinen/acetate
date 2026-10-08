package io.github.allinen.acetate

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.dp
import androidx.compose.material3.MaterialTheme
import uniffi.acetate_ffi.MarkInfo
import uniffi.acetate_ffi.MarkOn
import kotlin.math.cos
import kotlin.math.sin

/** The user's star, rating and play count on something; read again when [Model.marksVersion] moves. */
@Composable
fun rememberMark(model: Model, id: String): MarkInfo =
    remember(id, model.marksVersion, model.catalogVersion) { model.core.mark(id) }

private fun DrawScope.starPath(): Path {
    val c = Offset(size.width / 2, size.height * 0.54f)
    val outer = size.minDimension * 0.5f
    val inner = outer * 0.42f
    return Path().apply {
        for (i in 0 until 10) {
            val r = if (i % 2 == 0) outer else inner
            val a = Math.PI * i / 5 - Math.PI / 2
            val x = c.x + (r * cos(a)).toFloat()
            val y = c.y + (r * sin(a)).toFloat()
            if (i == 0) moveTo(x, y) else lineTo(x, y)
        }
        close()
    }
}

/** A star, `fill` of it (0, 0.5 or 1) coloured from the left. */
@Composable
private fun StarIcon(fill: Float, color: Color, size: Int) {
    Canvas(Modifier.size(size.dp)) {
        val path = starPath()
        drawPath(path, color.copy(alpha = 0.5f), style = Stroke(width = this.size.minDimension * 0.07f))
        if (fill > 0f) clipRect(right = this.size.width * fill) { drawPath(path, color) }
    }
}

/** Starred or not: tap to change. */
@Composable
fun StarToggle(model: Model, on: MarkOn, id: String, size: Int = 28) {
    val starred = rememberMark(model, id).starred
    val color = MaterialTheme.colorScheme.primary
    Box(Modifier.clickable { model.core.setStarred(on, id, !starred) }.padding(8.dp)) {
        StarIcon(if (starred) 1f else 0f, color, size)
    }
}

/** Five stars in halves: tap the left half of one for a half star; tap the rating again to clear it. */
@Composable
fun RatingBar(model: Model, on: MarkOn, id: String, size: Int = 32) {
    val rating = rememberMark(model, id).rating.toInt()
    val color = MaterialTheme.colorScheme.primary
    Row(
        Modifier.pointerInput(rating, id) {
            detectTapGestures { at ->
                val each = size.dp.toPx()
                val star = (at.x / each).toInt().coerceIn(0, 4)
                val half = if (at.x - star * each < each / 2) 1 else 2
                val value = star * 2 + half
                model.core.setRating(on, id, (if (value == rating) 0 else value).toShort())
            }
        },
    ) {
        for (i in 0 until 5) {
            val fill = ((rating - i * 2).coerceIn(0, 2)) / 2f
            StarIcon(fill, color, size)
        }
    }
}
