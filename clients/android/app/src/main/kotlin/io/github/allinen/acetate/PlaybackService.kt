package io.github.allinen.acetate

import android.content.ComponentName
import android.content.Context
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.ForwardingPlayer
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.database.StandaloneDatabaseProvider
import androidx.media3.datasource.DataSource
import androidx.media3.datasource.DataSpec
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.datasource.cache.CacheDataSource
import androidx.media3.datasource.cache.LeastRecentlyUsedCacheEvictor
import androidx.media3.datasource.cache.SimpleCache
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.DefaultLoadControl
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.MediaLibraryService
import androidx.media3.session.MediaSession
import okhttp3.OkHttpClient
import uniffi.acetate_ffi.MediaRequest
import uniffi.acetate_ffi.Playback
import uniffi.acetate_ffi.PlayerEngine
import java.io.File

/**
 * The sound. Media3's ExoPlayer plays, but the core decides what: ExoPlayer only ever
 * holds the current track and the one after it (the core's "next slot", which is what
 * makes playback gapless), and every button (the app's, the notification's, the lock
 * screen's, a headset's) goes to the core, which answers by calling [ExoEngine].
 */
class PlaybackService : MediaLibraryService() {
    private var session: MediaLibrarySession? = null
    private lateinit var player: ExoPlayer
    private val main = Handler(Looper.getMainLooper())
    private val ticker = object : Runnable {
        override fun run() {
            report()
            main.postDelayed(this, 500)
        }
    }

    override fun onCreate() {
        super.onCreate()
        val playback = (application as AcetateApp).model.playback

        val upstream = ResolvingDataSource.Factory(
            OkHttpDataSource.Factory(OkHttpClient()),
            // The one server's login, put on every request (audio, and covers for the notification).
            ResolvingDataSource.Resolver { spec: DataSpec ->
                val auth = Auth.header
                if (auth == null) spec else spec.withAdditionalHeaders(mapOf("Authorization" to auth))
            },
        )
        val cached: DataSource.Factory = CacheDataSource.Factory()
            .setCache(cache(this))
            .setUpstreamDataSourceFactory(upstream)
            .setFlags(CacheDataSource.FLAG_IGNORE_CACHE_ON_ERROR)

        // The equalizer sits in the audio sink, so every track passes through it.
        val renderers = object : androidx.media3.exoplayer.DefaultRenderersFactory(this) {
            override fun buildAudioSink(context: Context, enableFloatOutput: Boolean, enableAudioTrackPlaybackParams: Boolean) =
                androidx.media3.exoplayer.audio.DefaultAudioSink.Builder(context)
                    .setEnableFloatOutput(enableFloatOutput)
                    .setEnableAudioTrackPlaybackParams(enableAudioTrackPlaybackParams)
                    .setAudioProcessors(arrayOf(EqProcessor()))
                    .build()
        }
        player = ExoPlayer.Builder(this, renderers)
            // Files on the device (downloads) are read from there, the rest through the cache.
            .setMediaSourceFactory(DefaultMediaSourceFactory(androidx.media3.datasource.DefaultDataSource.Factory(this, cached)))
            .setAudioAttributes(
                AudioAttributes.Builder().setUsage(C.USAGE_MEDIA).setContentType(C.AUDIO_CONTENT_TYPE_MUSIC).build(),
                true,
            )
            .setHandleAudioBecomingNoisy(true)
            .setWakeMode(C.WAKE_MODE_NETWORK)
            .setLoadControl(DefaultLoadControl.Builder().setBufferDurationsMs(30_000, 120_000, 1_500, 3_000).build())
            .build()
        Output.attach(player)

        player.addListener(object : Player.Listener {
            override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
                // Only going on by itself is the core's news; what the core asked for it knows.
                if (reason == Player.MEDIA_ITEM_TRANSITION_REASON_AUTO) {
                    // What played is let go (the core fills the slot after it again).
                    if (player.currentMediaItemIndex > 0) player.removeMediaItems(0, player.currentMediaItemIndex)
                    playback.onTransition()
                }
            }

            override fun onPlaybackStateChanged(state: Int) {
                if (state == Player.STATE_ENDED) playback.onEnded()
                report()
            }

            override fun onIsPlayingChanged(isPlaying: Boolean) {
                report()
                if (isPlaying) {
                    main.removeCallbacks(ticker)
                    main.post(ticker)
                } else {
                    main.removeCallbacks(ticker)
                }
            }

            override fun onPlayerError(error: PlaybackException) {
                Log.e("Acetate", "playback: ${error.errorCodeName}: ${error.message}", error)
                // A track that can't be played: on to the next.
                playback.next()
            }
        })

        engine = ExoEngine(player)
        playback.attach(engine!!)
        this.playback = playback

        session = MediaLibrarySession.Builder(this, CorePlayer(player, playback), object : MediaLibrarySession.Callback {})
            .setBitmapLoader(androidx.media3.datasource.DataSourceBitmapLoader.Builder(this).setDataSourceFactory(androidx.media3.datasource.DefaultDataSource.Factory(this, upstream)).build())
            .build()
    }

    private var engine: ExoEngine? = null
    private var playback: Playback? = null

    /** Tells the core where playback is. */
    private fun report() {
        val playing = player.playbackState != Player.STATE_IDLE && player.playbackState != Player.STATE_ENDED && player.mediaItemCount > 0
        playback?.onPosition(player.currentPosition.coerceAtLeast(0).toULong(), playing, !player.playWhenReady)
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaLibrarySession? = session

    override fun onTaskRemoved(rootIntent: android.content.Intent?) {
        // Swiped away from the recents while paused: nothing keeps the service.
        if (!player.playWhenReady || player.mediaItemCount == 0) stopSelf()
    }

    override fun onDestroy() {
        main.removeCallbacks(ticker)
        session?.run {
            release()
            session = null
        }
        Output.attach(null)
        player.release()
        super.onDestroy()
    }

    companion object {
        private var cache: SimpleCache? = null

        /** What was played, kept (one cache per process). */
        @Synchronized
        private fun cache(context: Context): SimpleCache =
            cache ?: SimpleCache(
                File(context.cacheDir, "media"),
                LeastRecentlyUsedCacheEvictor(512L * 1024 * 1024),
                StandaloneDatabaseProvider(context),
            ).also { cache = it }

        fun token(context: Context) =
            androidx.media3.session.SessionToken(context, ComponentName(context, PlaybackService::class.java))
    }
}

/** The login the player's requests carry (there's one server). */
object Auth {
    @Volatile
    var header: String? = null
}

/** The core's calls, handed to ExoPlayer's own thread. */
class ExoEngine(private val player: ExoPlayer) : PlayerEngine {
    private val main = Handler(Looper.getMainLooper())

    private fun item(media: MediaRequest): MediaItem {
        // A file on the device needs no login (and the last one's stays for what comes after).
        if (media.authorization.isNotEmpty()) Auth.header = media.authorization
        val metadata = MediaMetadata.Builder()
            .setTitle(media.title)
            .setArtist(media.artist)
            .setAlbumTitle(media.album)
            .setArtworkUri(media.cover?.let { Uri.parse(it.url) })
            .build()
        return MediaItem.Builder()
            .setMediaId(media.trackId)
            .setUri(media.url)
            .setMediaMetadata(metadata)
            .build()
    }

    override fun play(media: MediaRequest, atMs: ULong, paused: Boolean) {
        val item = item(media)
        main.post {
            player.setMediaItem(item, atMs.toLong())
            player.prepare()
            player.playWhenReady = !paused
        }
    }

    override fun setNext(media: MediaRequest?) {
        val item = media?.let(::item)
        main.post {
            val after = player.currentMediaItemIndex + 1
            if (player.mediaItemCount > after) player.removeMediaItems(after, player.mediaItemCount)
            if (item != null && player.mediaItemCount > 0) player.addMediaItem(item)
        }
    }

    override fun setPaused(paused: Boolean) {
        main.post { player.playWhenReady = !paused }
    }

    override fun seek(toMs: ULong) {
        main.post { player.seekTo(toMs.toLong()) }
    }

    override fun skip() {
        main.post { player.seekToNextMediaItem() }
    }

    override fun stop() {
        main.post {
            player.stop()
            player.clearMediaItems()
        }
    }
}

/**
 * What the system's controls (notification, lock screen, Bluetooth, headset) talk to: the
 * buttons go to the core, whose answer comes back through [ExoEngine]. Next and previous
 * are always offered; the core knows what they mean.
 */
class CorePlayer(player: Player, private val playback: Playback) : ForwardingPlayer(player) {
    override fun getAvailableCommands(): Player.Commands =
        super.getAvailableCommands().buildUpon()
            .add(COMMAND_SEEK_TO_NEXT)
            .add(COMMAND_SEEK_TO_PREVIOUS)
            .add(COMMAND_SEEK_TO_NEXT_MEDIA_ITEM)
            .add(COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM)
            .build()

    override fun isCommandAvailable(command: Int): Boolean = availableCommands.contains(command)

    override fun hasNextMediaItem(): Boolean = true

    override fun hasPreviousMediaItem(): Boolean = true

    override fun play() = playback.setPaused(false)

    override fun pause() = playback.setPaused(true)

    override fun setPlayWhenReady(playWhenReady: Boolean) = playback.setPaused(!playWhenReady)

    override fun seekToNext() = playback.next()

    override fun seekToNextMediaItem() = playback.next()

    override fun seekToPrevious() = playback.previous()

    override fun seekToPreviousMediaItem() = playback.previous()

    override fun seekTo(positionMs: Long) = playback.seek(positionMs.coerceAtLeast(0).toULong())

    override fun seekTo(mediaItemIndex: Int, positionMs: Long) = playback.seek(positionMs.coerceAtLeast(0).toULong())
}
