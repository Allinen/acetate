package io.github.allinen.acetate

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.widget.Toast
import java.io.File

/**
 * Putting the downloaded APK of the server's release in place of this app. The core downloads
 * it and checks its checksum (`Core.downloadUpdate`); the system's installer does the rest,
 * and only accepts it if it is signed like this app. It asks the user to confirm (and, the
 * first time, to allow installing from Acetate). Always forwards: the core never offers an
 * older release.
 */
object AppUpdate {
    fun install(context: Context, path: String) {
        val apk = File(path)
        val installer = context.packageManager.packageInstaller
        val id = installer.createSession(PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL))
        installer.openSession(id).use { session ->
            apk.inputStream().use { input ->
                session.openWrite("acetate.apk", 0, apk.length()).use { out ->
                    input.copyTo(out)
                    session.fsync(out)
                }
            }
            val result = PendingIntent.getBroadcast(
                context, id, Intent(context, UpdateReceiver::class.java).setPackage(context.packageName),
                // (The installer fills the intent in with the outcome, so it must be mutable.)
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
            )
            session.commit(result.intentSender)
        }
        // The session holds its own copy.
        apk.delete()
    }
}

/** What the installer says about an update: its confirmation to show, or why it failed. */
class UpdateReceiver : BroadcastReceiver() {
    @Suppress("DEPRECATION")
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                val confirm = intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT) ?: return
                context.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
            // (Installed: the app is replaced and restarts itself.)
            PackageInstaller.STATUS_SUCCESS -> {}
            else -> Toast.makeText(
                context, "The update wasn't installed: " + (intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE) ?: "unknown reason"), Toast.LENGTH_LONG,
            ).show()
        }
    }
}
