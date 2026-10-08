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
        enableEdgeToEdge()
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
        setContent { AcetateTheme { Root(model) } }
    }
}
