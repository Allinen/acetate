package io.github.allinen.acetate

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue

/** The choices for streaming quality, in kbps (0: the file as it is). */
val QUALITIES = listOf(0, 320, 192, 128, 96)

fun qualityName(kbps: Int) = if (kbps == 0) "Original" else "$kbps kbps"

/**
 * Which quality to stream at: one choice on Wi-Fi (an unmetered connection), one on
 * metered ones (mobile data). Applies to what is loaded next, as the connection changes.
 */
class Quality(context: Context, private val model: Model) {
    private val prefs = context.getSharedPreferences("quality", Context.MODE_PRIVATE)

    var unmetered by mutableIntStateOf(prefs.getInt("unmetered", 0))
        private set
    var metered by mutableIntStateOf(prefs.getInt("metered", 128))
        private set

    /** The connection in use is metered (assumed until the system says otherwise). */
    private var onMetered = true

    init {
        val connectivity = context.getSystemService(ConnectivityManager::class.java)
        connectivity.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
                val now = !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
                model.post { onMetered = now; apply() }
            }
        })
    }

    fun chooseUnmetered(kbps: Int) {
        unmetered = kbps
        prefs.edit().putInt("unmetered", kbps).apply()
        apply()
    }

    fun chooseMetered(kbps: Int) {
        metered = kbps
        prefs.edit().putInt("metered", kbps).apply()
        apply()
    }

    private fun apply() {
        val kbps = if (onMetered) metered else unmetered
        model.playback.setQuality(if (kbps == 0) null else kbps.toUInt())
    }
}
