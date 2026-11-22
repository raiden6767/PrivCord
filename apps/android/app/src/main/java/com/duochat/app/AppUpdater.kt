package com.duochat.app

import android.annotation.SuppressLint
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Build
import com.duochat.app.data.AppUpdate
import com.duochat.app.data.UpdateInfo
import androidx.compose.runtime.mutableStateOf
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/** Bumped each time the app comes to the front, so the start-up check can run again; remembers when it last ran. */
/** The state of a running or finished update download, shared by [UpdateService] and the buttons that start it. */
data class UpdateDownload(val busy: Boolean = false, val percent: Int = 0, val message: String? = null)

object UpdateSignal {
    val download = mutableStateOf(UpdateDownload())
    val resumes = mutableStateOf(0)
    @Volatile var lastCheck = 0L
}

/**
 * In-app updates without a store: read the small `android.json` from our release, and if there is a newer build, stream
 * its APK straight into Android's installer (nothing is written to a file of ours). The user confirms the install and
 * Android refuses it unless it is signed with the same key as the installed app, so a swapped file cannot install.
 * The checksum only catches a damaged download.
 */
object AppUpdater {
    /** Failure if the server could not be reached or the file is not ours; success(null) when already up to date. */
    suspend fun latest(): Result<UpdateInfo?> = withContext(Dispatchers.IO) {
        runCatching {
            // Only the 64-bit APK is published; a 32-bit phone cannot install it, so it is not offered.
            if (!Build.SUPPORTED_ABIS.contains("arm64-v8a")) return@runCatching null
            val conn = open(AppUpdate.INFO_URL)
            val body = conn.inputStream.use { it.readBytes().decodeToString() }
            val info = AppUpdate.parse(body) ?: error("The update information is not valid.")
            info.takeIf { AppUpdate.isNewer(it, BuildConfig.VERSION_CODE) }
        }
    }

    /** Downloads the APK into an install session and hands it to Android, which asks the user to confirm. */
    suspend fun install(context: Context, info: UpdateInfo, progress: (Int) -> Unit): Result<Unit> = withContext(Dispatchers.IO) {
        runCatching {
            val installer = context.packageManager.packageInstaller
            val conn = open(info.url)
            val length = conn.contentLengthLong
            val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
            if (length > 0) params.setSize(length)
            val id = installer.createSession(params)
            try {
                installer.openSession(id).use { session ->
                    val digest = MessageDigest.getInstance("SHA-256")
                    conn.inputStream.use { input ->
                        session.openWrite("update.apk", 0, length).use { out ->
                            val buffer = ByteArray(64 * 1024)
                            var done = 0L
                            while (true) {
                                val n = input.read(buffer)
                                if (n < 0) break
                                out.write(buffer, 0, n)
                                digest.update(buffer, 0, n)
                                done += n
                                if (length > 0) progress((done * 100 / length).toInt())
                            }
                            session.fsync(out)
                        }
                    }
                    val got = digest.digest().joinToString("") { "%02x".format(it) }
                    check(got == info.sha256) { "The download did not match its checksum. Try again." }
                    val result = Intent(context, InstallResultReceiver::class.java).setPackage(context.packageName)
                    @SuppressLint("InlinedApi")
                    val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE
                    session.commit(PendingIntent.getBroadcast(context, id, result, flags).intentSender)
                }
            } catch (e: Throwable) {
                runCatching { installer.abandonSession(id) }
                throw e
            }
        }
    }

    private fun open(url: String): HttpURLConnection =
        (URL(url).openConnection() as HttpURLConnection).apply {
            connectTimeout = 15_000
            readTimeout = 30_000
            instanceFollowRedirects = true
            setRequestProperty("User-Agent", "PrivCord")
        }
}

/** Android calls this when the installer needs the user to confirm: it opens that confirmation screen. */
class InstallResultReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, -1) != PackageInstaller.STATUS_PENDING_USER_ACTION) return
        val confirm = if (Build.VERSION.SDK_INT >= 33) {
            intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT)
        } ?: return
        // From the background Android blocks opening a screen, so there is always a tap-to-install notification as well.
        runCatching { context.startActivity(Intent(confirm).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
        runCatching {
            UpdateService.createChannel(context)
            val tap = PendingIntent.getActivity(
                context, 1, Intent(confirm).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            val ready = androidx.core.app.NotificationCompat.Builder(context, UpdateService.CHANNEL_ID)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle("PrivCord update is ready")
                .setContentText("Tap to install it")
                .setContentIntent(tap)
                .setAutoCancel(true)
                .setPriority(androidx.core.app.NotificationCompat.PRIORITY_HIGH)
                .build()
            context.getSystemService(android.app.NotificationManager::class.java)?.notify(UpdateService.READY_ID, ready)
        }
    }
}
