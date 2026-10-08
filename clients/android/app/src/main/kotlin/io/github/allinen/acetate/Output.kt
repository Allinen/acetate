package io.github.allinen.acetate

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import androidx.media3.exoplayer.ExoPlayer
import kotlin.math.pow

/**
 * A gain reduction for the app's own output, in dB below full (0 to 30), on top of the
 * phone's volume: for headphones that are too loud at the lowest volume step.
 */
object Output {
    private var prefs: android.content.SharedPreferences? = null
    private var player: ExoPlayer? = null

    var reduction by mutableIntStateOf(0)
        private set

    fun init(context: Context) {
        prefs = context.getSharedPreferences("output", Context.MODE_PRIVATE)
        reduction = prefs?.getInt("reduction_db", 0) ?: 0
    }

    fun set(db: Int) {
        reduction = db.coerceIn(0, 30)
        prefs?.edit()?.putInt("reduction_db", reduction)?.apply()
        apply()
    }

    /** The service's player (null: it's gone). Must be called on the main thread. */
    fun attach(player: ExoPlayer?) {
        this.player = player
        apply()
    }

    private fun apply() {
        player?.volume = 10f.pow(-reduction / 20f)
    }
}
