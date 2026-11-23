package com.duochat.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import com.duochat.app.data.UpdateInfo
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * Downloads an update in a foreground service, so it carries on when the person switches to another app (a plain
 * coroutine in the screen was stopped by Android within seconds). Progress goes to [UpdateSignal] (the screen) and to
 * a notification; when the installer needs the person's confirmation, [InstallResultReceiver] posts a tap-to-install one.
 */
class UpdateService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Android kills the app unless startForeground() follows startForegroundService() within seconds, whatever else
        // happens, so this comes first and no early return may skip it.
        createChannel(this)
        val first = notification(0)
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(NOTIFICATION_ID, first, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
            } else {
                startForeground(NOTIFICATION_ID, first)
            }
        } catch (_: Throwable) {
            // Refused: carry on as a plain download, which still works while the app stays open.
        }
        val info = intent?.let {
            UpdateInfo(
                it.getIntExtra("code", 0), it.getStringExtra("name").orEmpty(),
                it.getStringExtra("url").orEmpty(), it.getStringExtra("sha").orEmpty(),
            )
        }
        if (info == null || info.url.isEmpty()) {
            UpdateSignal.download.value = UpdateDownload()
            stopSelf(startId)
            return START_NOT_STICKY
        }
        // One download at a time. This is the service's own flag: the screen sets UpdateSignal.download to busy before
        // it starts the service, so that cannot tell whether a download is really running. A second start while one
        // runs does nothing; the first keeps the service alive.
        if (!running.compareAndSet(false, true)) return START_NOT_STICKY
        UpdateSignal.download.value = UpdateDownload(busy = true)
        val manager = getSystemService(NotificationManager::class.java)
        scope.launch {
            var last = -1
            // A dropped connection (the phone changed network, or the system cut the socket) starts the download again.
            var result: Result<Unit> = Result.failure(IllegalStateException("The update failed."))
            for (attempt in 1..3) {
                result = AppUpdater.install(applicationContext, info) { p ->
                    UpdateSignal.download.value = UpdateDownload(busy = true, percent = p)
                    if (p != last) {
                        last = p
                        manager?.notify(NOTIFICATION_ID, notification(p))
                    }
                }
                val e = result.exceptionOrNull()
                if (e == null || e is IllegalStateException) break // done, or the file itself is wrong: no point retrying
                kotlinx.coroutines.delay(2_000L * attempt)
            }
            result
                .onSuccess {
                    UpdateSignal.download.value = UpdateDownload(message = "Downloaded. Confirm the install when Android asks.")
                }
                .onFailure {
                    UpdateSignal.download.value = UpdateDownload(message = "The download was interrupted. Check your connection and tap Install again.")
                }
            running.set(false)
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
        return START_NOT_STICKY
    }

    /** Android 15 stops a "data sync" service after six hours a day and expects it to stop itself when told. */
    override fun onTimeout(startId: Int, fgsType: Int) {
        UpdateSignal.download.value = UpdateDownload(message = "The download took too long. Tap Install to try again.")
        stopSelf()
    }

    override fun onDestroy() {
        scope.coroutineContext[kotlinx.coroutines.Job]?.cancel()
        super.onDestroy()
    }

    private fun notification(percent: Int): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("Downloading the PrivCord update")
            .setContentText("$percent%")
            .setProgress(100, percent, false)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(open)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    companion object {
        private val running = java.util.concurrent.atomic.AtomicBoolean(false)
        const val CHANNEL_ID = "privcord_updates"
        const val NOTIFICATION_ID = 9002
        const val READY_ID = 9003

        fun createChannel(context: Context) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
            context.getSystemService(NotificationManager::class.java)?.createNotificationChannel(
                NotificationChannel(CHANNEL_ID, "App updates", NotificationManager.IMPORTANCE_LOW),
            )
        }

        fun start(context: Context, info: UpdateInfo) {
            val intent = Intent(context, UpdateService::class.java)
                .putExtra("code", info.versionCode).putExtra("name", info.versionName)
                .putExtra("url", info.url).putExtra("sha", info.sha256)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) context.startForegroundService(intent) else context.startService(intent)
        }
    }
}
