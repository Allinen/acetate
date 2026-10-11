package io.github.allinen.acetate

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.BaseAudioProcessor
import androidx.media3.common.C
import java.nio.ByteBuffer
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.pow
import kotlin.math.sin

/**
 * The equalizer, as on the desktop: ten peaking filters, one per octave (31 Hz to 16 kHz),
 * each moved up to 12 dB either way, with the whole sound lowered by the largest boost so
 * that boosting a band can't clip. The settings are kept on the phone; the filters run in
 * [EqProcessor], which the player's audio sink puts every track through.
 */
object Equalizer {
    val BANDS = floatArrayOf(31f, 62f, 125f, 250f, 500f, 1000f, 2000f, 4000f, 8000f, 16000f)
    const val RANGE_DB = 12

    private var prefs: android.content.SharedPreferences? = null

    var enabled by mutableStateOf(false)
        private set

    /** Gains in dB, one per band. */
    val gains = mutableStateListOf<Int>().apply { repeat(BANDS.size) { add(0) } }

    /** What the processor reads: replaced whole on every change, so it never sees half of one. */
    @Volatile
    var settings = Settings(false, DoubleArray(BANDS.size), 0)
        private set

    class Settings(val enabled: Boolean, val gains: DoubleArray, val version: Int)

    fun init(context: Context) {
        val p = context.getSharedPreferences("equalizer", Context.MODE_PRIVATE)
        prefs = p
        enabled = p.getBoolean("enabled", false)
        val saved = p.getString("gains", "")!!.split(',').mapNotNull { it.toIntOrNull() }
        if (saved.size == BANDS.size) saved.forEachIndexed { i, g -> gains[i] = g.coerceIn(-RANGE_DB, RANGE_DB) }
        publish()
    }

    fun enable(on: Boolean) {
        enabled = on
        save()
    }

    fun set(band: Int, db: Int) {
        gains[band] = db.coerceIn(-RANGE_DB, RANGE_DB)
        save()
    }

    fun flatten() {
        gains.indices.forEach { gains[it] = 0 }
        save()
    }

    /** How much the sound is lowered (dB, 0 or more) to make room for the boosts. */
    fun headroomDb(): Int = gains.maxOrNull()?.coerceAtLeast(0) ?: 0

    private fun save() {
        prefs?.edit()?.putBoolean("enabled", enabled)?.putString("gains", gains.joinToString(","))?.apply()
        publish()
    }

    private fun publish() {
        settings = Settings(enabled, DoubleArray(BANDS.size) { gains[it].toDouble() }, settings.version + 1)
    }
}

/** One peaking filter (RBJ cookbook), coefficients divided by a0. */
private class Biquad(rate: Double, centre: Double, gainDb: Double) {
    val b0: Double
    val b1: Double
    val b2: Double
    val a1: Double
    val a2: Double

    init {
        val a = 10.0.pow(gainDb / 40.0)
        val w = 2.0 * PI * centre / rate
        val alpha = sin(w) / (2.0 * 1.414) // one octave between bands
        val a0 = 1.0 + alpha / a
        b0 = (1.0 + alpha * a) / a0
        b1 = -2.0 * cos(w) / a0
        b2 = (1.0 - alpha * a) / a0
        a1 = b1
        a2 = (1.0 - alpha / a) / a0
    }
}

/** Passes 16-bit or float PCM through the equalizer; other formats go by untouched. */
class EqProcessor : BaseAudioProcessor() {
    private var version = -1
    private var float = false
    private var filters = emptyList<Pair<Int, Biquad>>()
    private var preamp = 1.0

    /** Transposed direct form II memory: channel × band × (z1, z2). */
    private var memory = DoubleArray(0)

    override fun onConfigure(input: AudioProcessor.AudioFormat): AudioProcessor.AudioFormat {
        if (input.encoding != C.ENCODING_PCM_16BIT && input.encoding != C.ENCODING_PCM_FLOAT) return AudioProcessor.AudioFormat.NOT_SET
        float = input.encoding == C.ENCODING_PCM_FLOAT
        version = -1
        return input
    }

    override fun onFlush() {
        // What the filters remember is from somewhere else in the track.
        memory = DoubleArray(inputAudioFormat.channelCount * Equalizer.BANDS.size * 2)
        version = -1
    }

    /** Takes the settings as they are now. */
    private fun update() {
        val s = Equalizer.settings
        if (s.version == version) return
        version = s.version
        val rate = inputAudioFormat.sampleRate.toDouble()
        filters = if (!s.enabled) emptyList() else Equalizer.BANDS.indices
            .filter { kotlin.math.abs(s.gains[it]) >= 0.05 && Equalizer.BANDS[it] < rate * 0.45 }
            .map { it to Biquad(rate, Equalizer.BANDS[it].toDouble(), s.gains[it]) }
        preamp = if (s.enabled) 10.0.pow(-(s.gains.maxOrNull() ?: 0.0).coerceAtLeast(0.0) / 20.0) else 1.0
    }

    override fun queueInput(inputBuffer: ByteBuffer) {
        update()
        val size = inputBuffer.remaining()
        if (size == 0) return
        val out = replaceOutputBuffer(size)
        if (filters.isEmpty()) {
            out.put(inputBuffer)
        } else {
            val channels = inputAudioFormat.channelCount
            var channel = 0
            while (inputBuffer.hasRemaining()) {
                var x = (if (float) inputBuffer.getFloat().toDouble() else inputBuffer.getShort() / 32768.0) * preamp
                val base = channel * Equalizer.BANDS.size * 2
                for ((band, f) in filters) {
                    val m = base + band * 2
                    val y = f.b0 * x + memory[m]
                    memory[m] = f.b1 * x - f.a1 * y + memory[m + 1]
                    memory[m + 1] = f.b2 * x - f.a2 * y
                    x = y
                }
                if (float) out.putFloat(x.toFloat()) else out.putShort((x * 32768.0).coerceIn(-32768.0, 32767.0).toInt().toShort())
                channel = (channel + 1) % channels
            }
        }
        out.flip()
    }
}
