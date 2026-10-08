package io.github.allinen.acetate

import android.content.Context
import android.media.AudioDeviceInfo
import android.media.AudioManager
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue

/**
 * How far to move the lyrics from the player's position, in milliseconds (positive:
 * sooner). What the player says it has played isn't quite what's heard: a Bluetooth
 * headset adds a delay the phone knows only roughly, so there's one setting for when one
 * is connected and one for everything else.
 */
object LyricsTiming {
    private var prefs: android.content.SharedPreferences? = null
    private var audio: AudioManager? = null

    var bluetoothMs by mutableIntStateOf(0)
        private set
    var otherMs by mutableIntStateOf(0)
        private set

    fun init(context: Context) {
        prefs = context.getSharedPreferences("lyrics", Context.MODE_PRIVATE)
        audio = context.getSystemService(AudioManager::class.java)
        bluetoothMs = prefs?.getInt("bluetooth_ms", 0) ?: 0
        otherMs = prefs?.getInt("other_ms", 0) ?: 0
    }

    fun setBluetooth(ms: Int) {
        bluetoothMs = ms
        prefs?.edit()?.putInt("bluetooth_ms", ms)?.apply()
    }

    fun setOther(ms: Int) {
        otherMs = ms
        prefs?.edit()?.putInt("other_ms", ms)?.apply()
    }

    /** Whether a Bluetooth headset or speaker is connected for music. */
    fun bluetoothConnected(): Boolean =
        audio?.getDevices(AudioManager.GET_DEVICES_OUTPUTS)?.any {
            it.type == AudioDeviceInfo.TYPE_BLUETOOTH_A2DP || it.type == AudioDeviceInfo.TYPE_BLE_HEADSET ||
                it.type == AudioDeviceInfo.TYPE_BLE_SPEAKER
        } == true

    /** The offset for the way it's listened to now. */
    fun offsetMs(): Int = if (bluetoothConnected()) bluetoothMs else otherMs
}
