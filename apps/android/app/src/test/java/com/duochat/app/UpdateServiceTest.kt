package com.duochat.app

import android.app.Application
import android.content.Intent
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * Android kills the app if a service started with startForegroundService() does not call startForeground() in time
 * (ForegroundServiceDidNotStartInTimeException). That crashed Install on a phone, so every way the service can be
 * started is checked here, on the JVM.
 */
@RunWith(AndroidJUnit4::class)
@Config(sdk = [35], application = Application::class)
class UpdateServiceTest {
    private fun intent(url: String) = Intent(ApplicationProvider.getApplicationContext<Application>(), UpdateService::class.java)
        .putExtra("code", 99).putExtra("name", "9.9.9").putExtra("url", url).putExtra("sha", "00")

    private fun start(i: Intent) = Robolectric.buildService(UpdateService::class.java, i).create().startCommand(0, 1).get()

    @Test fun itGoesForegroundEvenWhenTheScreenAlreadyMarkedTheDownloadBusy() {
        // What the Install button does just before it starts the service: this is the exact case that crashed.
        UpdateSignal.download.value = UpdateDownload(busy = true)
        val service = start(intent("http://127.0.0.1:9/PrivCord.apk"))
        assertNotNull("startForeground() was not called", shadowOf(service).lastForegroundNotification)
    }

    @Test fun itGoesForegroundEvenWithNothingToDownload() {
        UpdateSignal.download.value = UpdateDownload(busy = true)
        val service = start(intent(""))
        assertNotNull("startForeground() was not called", shadowOf(service).lastForegroundNotification)
        assertFalse("the screen was left showing a download that never started", UpdateSignal.download.value.busy)
    }

    @Test fun aSecondStartWhileDownloadingDoesNotStopTheService() {
        start(intent("http://127.0.0.1:9/PrivCord.apk"))
        val second = start(intent("http://127.0.0.1:9/PrivCord.apk"))
        assertNotNull(shadowOf(second).lastForegroundNotification)
        assertFalse("a second tap stopped the service under the running download", shadowOf(second).isStoppedBySelf)
        assertTrue(true)
    }
}
