package io.github.allinen.acetate

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.lifecycle.lifecycleScope
import android.util.Log
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(
            statusBarStyle = androidx.activity.SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
            navigationBarStyle = androidx.activity.SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
        )
        val model = (application as AcetateApp).model
        // Debug builds: `adb shell am start -n …/.MainActivity --es probe https://…` asks
        // that address (no login) and logs what came back (`adb logcat -s Acetate`).
        if (BuildConfig.DEBUG) intent.getStringExtra("probe")?.let { address ->
            lifecycleScope.launch {
                val start = android.os.SystemClock.elapsedRealtime()
                val answer = model.core.reach(address)
                Log.i("Acetate", "probe $address: $answer (${android.os.SystemClock.elapsedRealtime() - start} ms)")
            }
        }
        // The player's service starts with the app, so the core's queue (brought back
        // paused) has a player to go to before anything is played.
        androidx.media3.session.MediaController.Builder(this, PlaybackService.token(this)).buildAsync()
        // Debug builds: `--es play <album id>` plays an album from adb.
        if (BuildConfig.DEBUG) intent.getStringExtra("play")?.let { album ->
            lifecycleScope.launch {
                while (!model.sync.haveCatalog) kotlinx.coroutines.delay(200)
                // `first`: the first album of the library.
                val id = if (album == "first") model.core.albums("", uniffi.acetate_ffi.AlbumOrder.TITLE).rows(0uL, 1uL)[0].id else album
                Log.i("Acetate", "playing album $id")
                model.playback.playAlbum(id, 0uL)
            }
        }
        if (android.os.Build.VERSION.SDK_INT >= 33) {
            requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1)
        }
        model.openLink(intent?.dataString)
        setContent { AcetateTheme { Root(model) } }
    }

    // A link opened while the app is running (it's one task: singleTask).
    override fun onNewIntent(intent: android.content.Intent) {
        super.onNewIntent(intent)
        (application as AcetateApp).model.openLink(intent.dataString)
    }
}
