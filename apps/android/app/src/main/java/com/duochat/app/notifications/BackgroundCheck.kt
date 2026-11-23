package com.duochat.app.notifications

import android.app.Application
import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
import com.duochat.app.data.SettingsRepository
import com.duochat.app.engine.EngineHost
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * A safety net for phones that stop the background service: about every 15 minutes (Android's shortest period) the system
 * wakes this check, which starts the chat engine for half a minute so it reaches the relays and collects the sealed
 * messages waiting there (they are kept for 7 days); the engine's own notifications show them. If the background service
 * is already running this finds the engine up and changes nothing. It sends no data to any push service: the phone asks
 * the relays, the relays are not told to wake the phone. Nothing here needs a Google service or a server of ours.
 */
class BackgroundCheck : JobService() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var waiting: Job? = null

    override fun onStartJob(params: JobParameters): Boolean {
        val app = application as Application
        val wanted = runCatching { SettingsRepository(app).settings.value.notificationsEnabled }.getOrDefault(false)
        if (!wanted) return false
        runCatching { EngineHost.ensureStarted(app) }.getOrNull() ?: return false
        waiting = scope.launch {
            delay(CHECK_MS)
            jobFinished(params, false)
        }
        return true
    }

    override fun onStopJob(params: JobParameters): Boolean {
        waiting?.cancel()
        // The system cut it short (no network, low battery): the next period tries again.
        return false
    }

    companion object {
        private const val JOB_ID = 4711
        const val PERIOD_MS = 15 * 60 * 1000L
        /** How long the engine is given to connect and collect what is waiting. */
        private const val CHECK_MS = 30_000L

        /** Makes sure the check is scheduled (it is kept across restarts). An existing schedule is left as it is. */
        fun schedule(context: Context) {
            runCatching {
                val scheduler = context.getSystemService(JobScheduler::class.java) ?: return
                if (scheduler.getPendingJob(JOB_ID) != null) return
                val job = JobInfo.Builder(JOB_ID, ComponentName(context, BackgroundCheck::class.java))
                    .setPeriodic(PERIOD_MS)
                    .setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
                    .setPersisted(true)
                    .build()
                scheduler.schedule(job)
            }
        }

        fun cancel(context: Context) {
            runCatching { context.getSystemService(JobScheduler::class.java)?.cancel(JOB_ID) }
        }
    }
}
