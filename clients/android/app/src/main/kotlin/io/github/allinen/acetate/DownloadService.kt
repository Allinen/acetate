package io.github.allinen.acetate

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Handler
import android.os.IBinder
import android.os.Looper

/**
 * Keeps the app alive while downloads run (the core does the fetching; this is the
 * notification that lets it go on with the screen off). It ends itself when nothing is left.
 */
class DownloadService : Service() {
    private val main = Handler(Looper.getMainLooper())
    private val tick = object : Runnable {
        override fun run() {
            val model = (application as AcetateApp).model
            val d = model.downloads
            // Nothing left, or the user stopped them: nothing keeps the app.
            if (d.queued == 0uL || d.paused) {
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
                return
            }
            getSystemService(NotificationManager::class.java).notify(ID, notification(d.done.toInt(), d.asked.toInt()))
            main.postDelayed(this, 1000)
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL, "Downloads", NotificationManager.IMPORTANCE_LOW))
        val model = (application as AcetateApp).model
        startForeground(ID, notification(model.downloads.done.toInt(), model.downloads.asked.toInt()), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        main.removeCallbacks(tick)
        main.post(tick)
        return START_NOT_STICKY
    }

    private fun notification(done: Int, asked: Int): Notification =
        Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle("Downloading")
            .setContentText(if (asked > 0) "$done of $asked" else "")
            .setProgress(asked, done.coerceAtMost(asked), asked == 0)
            .setOngoing(true)
            .setContentIntent(
                android.app.PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), android.app.PendingIntent.FLAG_IMMUTABLE),
            )
            .build()

    override fun onDestroy() {
        main.removeCallbacks(tick)
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    companion object {
        private const val CHANNEL = "downloads"
        private const val ID = 2

        fun start(context: Context) {
            context.startForegroundService(Intent(context, DownloadService::class.java))
        }
    }
}
