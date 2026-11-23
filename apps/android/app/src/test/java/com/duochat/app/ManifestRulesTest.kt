package com.duochat.app

import android.app.Application
import android.app.Service
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * Rules Android enforces on every component, checked against the real merged manifest for every service the app
 * declares, so a service added later is covered without anyone remembering to write a test for it. Each rule is one
 * that crashed an app (or would) on a real phone.
 */
@RunWith(AndroidJUnit4::class)
@Config(sdk = [35], application = Application::class)
class ManifestRulesTest {
    private val app = ApplicationProvider.getApplicationContext<Application>()

    @Suppress("DEPRECATION")
    private val ours: List<ServiceInfo> = (app.packageManager
        .getPackageInfo(app.packageName, PackageManager.GET_SERVICES).services ?: emptyArray())
        .filter { it.name.startsWith("com.duochat.app") }

    private val foreground get() = ours.filter { it.foregroundServiceType != 0 }

    /** Foreground service type bit -> the permission Android 14+ demands for it (SecurityException without it). */
    private val permissionFor = mapOf(
        ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC to "FOREGROUND_SERVICE_DATA_SYNC",
        ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK to "FOREGROUND_SERVICE_MEDIA_PLAYBACK",
        ServiceInfo.FOREGROUND_SERVICE_TYPE_PHONE_CALL to "FOREGROUND_SERVICE_PHONE_CALL",
        ServiceInfo.FOREGROUND_SERVICE_TYPE_LOCATION to "FOREGROUND_SERVICE_LOCATION",
        ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE to "FOREGROUND_SERVICE_CONNECTED_DEVICE",
        ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION to "FOREGROUND_SERVICE_MEDIA_PROJECTION",
        ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA to "FOREGROUND_SERVICE_CAMERA",
        ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE to "FOREGROUND_SERVICE_MICROPHONE",
        ServiceInfo.FOREGROUND_SERVICE_TYPE_HEALTH to "FOREGROUND_SERVICE_HEALTH",
        ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING to "FOREGROUND_SERVICE_REMOTE_MESSAGING",
        ServiceInfo.FOREGROUND_SERVICE_TYPE_SYSTEM_EXEMPTED to "FOREGROUND_SERVICE_SYSTEM_EXEMPTED",
        ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE to "FOREGROUND_SERVICE_SPECIAL_USE",
    )

    @Test fun theManifestListsOurServices() {
        assertTrue("no services found: the manifest was not read", ours.isNotEmpty())
        assertTrue("UpdateService is missing from the manifest", ours.any { it.name.endsWith(".UpdateService") })
    }

    @Test fun everyDeclaredServiceClassExists() {
        for (s in ours) Class.forName(s.name)
    }

    @Test fun everyForegroundServiceTypeHasItsPermission() {
        @Suppress("DEPRECATION")
        val granted = app.packageManager.getPackageInfo(app.packageName, PackageManager.GET_PERMISSIONS)
            .requestedPermissions.orEmpty().toSet()
        for (s in foreground) for ((bit, name) in permissionFor) {
            if (s.foregroundServiceType and bit != 0) {
                assertTrue("${s.name} declares a type that needs android.permission.$name", "android.permission.$name" in granted)
            }
        }
    }

    @Test fun everyDataSyncServiceHandlesTheAndroid15Timeout() {
        for (s in foreground.filter { it.foregroundServiceType and ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC != 0 }) {
            val has = Class.forName(s.name).declaredMethods.any { it.name == "onTimeout" }
            assertTrue("${s.name} is a data-sync service and must override onTimeout(int, int)", has)
        }
    }

    @Test fun everyForegroundServiceCallsStartForegroundOnEveryStart() {
        assertTrue("no foreground services found", foreground.isNotEmpty())
        for (s in foreground) {
            @Suppress("UNCHECKED_CAST")
            val cls = Class.forName(s.name) as Class<Service>
            // A bare start, and a start with an empty intent: neither may skip startForeground().
            for (intent in listOf(Intent(app, cls), Intent())) {
                val service = Robolectric.buildService(cls, intent).create().startCommand(0, 1).get()
                assertNotNull("${s.name} did not call startForeground() for $intent", shadowOf(service).lastForegroundNotification)
            }
        }
    }

    @Test fun everyReceiverSurvivesAnEmptyOrUnknownIntent() {
        @Suppress("DEPRECATION")
        val receivers = (app.packageManager.getPackageInfo(app.packageName, PackageManager.GET_RECEIVERS).receivers ?: emptyArray())
            .filter { it.name.startsWith("com.duochat.app") }
        assertTrue("no receivers found: the manifest was not read", receivers.isNotEmpty())
        for (r in receivers) {
            val receiver = Class.forName(r.name).getDeclaredConstructor().newInstance() as android.content.BroadcastReceiver
            for (intent in listOf(Intent(), Intent("com.duochat.app.NOT_A_REAL_ACTION"), Intent(Intent.ACTION_BOOT_COMPLETED))) {
                try {
                    receiver.onReceive(app, intent)
                } catch (e: Exception) {
                    throw AssertionError("${r.name} crashed on $intent", e)
                }
            }
        }
    }

    @Test fun noServiceIsExported() {
        for (s in ours) assertEquals("${s.name} is exported", false, s.exported)
    }
}
