package io.github.allinen.acetate

import android.view.SurfaceView
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.common.VideoSize
import androidx.media3.datasource.DataSpec
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import kotlinx.coroutines.delay
import okhttp3.OkHttpClient
import uniffi.acetate_ffi.ExtraRow
import uniffi.acetate_ffi.MarkOn

/** A video to watch. */
data class VideoRef(val file: String, val name: String)

/** "Extras (n)" when something has extras (stems, vocals, videos…): opens their list. */
@Composable
fun ExtrasButton(model: Model, on: MarkOn, id: String, title: String) {
    val extras = remember(on, id, model.catalogVersion) { model.core.extrasOf(on, id) }
    if (extras.isEmpty()) return
    var open by remember { mutableStateOf(false) }
    TextButton({ open = true }) { Text("Extras (${extras.size})") }
    if (open) ExtrasDialog(model, title, extras) { open = false }
}

@Composable
private fun ExtrasDialog(model: Model, title: String, extras: List<ExtraRow>, close: () -> Unit) {
    AlertDialog(
        onDismissRequest = close,
        title = { Text("Extras · $title", maxLines = 1) },
        text = {
            LazyColumn {
                items(extras.size, key = { extras[it].id }) { i ->
                    val e = extras[i]
                    Row(
                        Modifier.fillMaxWidth().clickable {
                            when {
                                e.videoFile != null -> model.video = VideoRef(e.videoFile!!, e.name)
                                e.audioFile != null -> model.playback.playExtra(e.audioFile!!, e.name, e.kind, e.durationMs)
                            }
                            close()
                        }.padding(vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Cover(model, e.thumb, 48)
                        Spacer(Modifier.width(12.dp))
                        Column(Modifier.weight(1f)) {
                            Text(e.name, maxLines = 1)
                            Text(
                                listOfNotNull(e.kind, if (e.videoFile != null) "video" else null, e.durationMs?.let(::duration)).joinToString(" · "),
                                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
            }
        },
        confirmButton = { TextButton(close) { Text("Close") } },
    )
}

/** A video, full screen, with its own player (the music is paused while it's open). */
@Composable
fun VideoScreen(model: Model, video: VideoRef) {
    BackHandler { model.video = null }
    val context = LocalContext.current
    val request = remember(video) { model.core.extraStream(video.file) }
    val player = remember(video) {
        val upstream = ResolvingDataSource.Factory(OkHttpDataSource.Factory(OkHttpClient())) { spec: DataSpec ->
            request?.let { spec.withAdditionalHeaders(mapOf("Authorization" to it.authorization)) } ?: spec
        }
        ExoPlayer.Builder(context).setMediaSourceFactory(DefaultMediaSourceFactory(upstream)).build()
    }
    var ratio by remember { mutableFloatStateOf(16f / 9f) }
    var playing by remember { mutableStateOf(true) }
    var position by remember { mutableLongStateOf(0L) }
    var length by remember { mutableLongStateOf(1L) }
    var dragging by remember { mutableStateOf<Float?>(null) }
    DisposableEffect(player) {
        model.playback.setPaused(true)
        val listener = object : Player.Listener {
            override fun onVideoSizeChanged(size: VideoSize) {
                if (size.height > 0) ratio = size.width * size.pixelWidthHeightRatio / size.height
            }

            override fun onIsPlayingChanged(isPlaying: Boolean) {
                playing = isPlaying
            }
        }
        player.addListener(listener)
        request?.let {
            player.setMediaItem(MediaItem.fromUri(it.url))
            player.prepare()
            player.playWhenReady = true
        }
        onDispose {
            player.removeListener(listener)
            player.release()
        }
    }
    LaunchedEffect(player) {
        while (true) {
            position = player.currentPosition
            length = player.duration.coerceAtLeast(1L)
            delay(250)
        }
    }
    Box(Modifier.fillMaxSize().background(Color.Black).safeDrawingPadding()) {
        Column(Modifier.fillMaxSize()) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                TextButton({ model.video = null }) { Text("Close") }
                Text(video.name, maxLines = 1, style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
            }
            Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                AndroidView(
                    { SurfaceView(it).also { view -> player.setVideoSurfaceView(view) } },
                    Modifier.fillMaxWidth().aspectRatio(ratio),
                )
            }
            Row(Modifier.fillMaxWidth().padding(8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                TextButton({ if (playing) player.pause() else player.play() }) { Text(if (playing) "Pause" else "Play") }
                Slider(
                    value = dragging ?: position.toFloat().coerceAtMost(length.toFloat()),
                    onValueChange = { dragging = it },
                    onValueChangeFinished = {
                        dragging?.let { player.seekTo(it.toLong()) }
                        dragging = null
                    },
                    valueRange = 0f..length.toFloat(),
                    modifier = Modifier.weight(1f),
                )
                Text(duration(position), style = MaterialTheme.typography.labelMedium)
            }
        }
    }
}
