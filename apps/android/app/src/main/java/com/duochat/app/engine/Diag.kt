package com.duochat.app.engine

import java.util.concurrent.ConcurrentLinkedQueue

/**
 * Where the screens and the platform write to the diagnostic log (which lives in the engine, on this device only).
 * Entries made before the engine is up wait here and go in once it is. Never put message text, names, keys or
 * addresses in an entry: the engine also blanks what looks like an identifier or an address, but do not rely on that.
 */
object Diag {
    private val early = ConcurrentLinkedQueue<Triple<String, String, String>>()
    @Volatile private var sink: ((String, String, String) -> Unit)? = null

    /** False while the person has the log turned off (nothing is passed on). */
    @Volatile var on: Boolean = true

    fun attach(write: (String, String, String) -> Unit) {
        sink = write
        while (true) {
            val e = early.poll() ?: break
            runCatching { write(e.first, e.second, e.third) }
        }
    }

    fun detach() {
        sink = null
    }

    /** [cat]: ui, nav, net, perm, ble, call, error, app. */
    fun log(cat: String, event: String, detail: String = "") {
        if (!on) return
        val write = sink
        if (write != null) {
            runCatching { write(cat, event, detail) }
        } else if (early.size < 300) {
            early.add(Triple(cat, event, detail))
        }
    }

    /** What the app says about itself at the top of a saved log (no names, no ids). */
    fun about(app: android.app.Application): String {
        val dm = app.resources.displayMetrics
        val cfg = app.resources.configuration
        return listOf(
            "PrivCord ${com.duochat.app.BuildConfig.VERSION_NAME}",
            "Android ${android.os.Build.VERSION.RELEASE} (API ${android.os.Build.VERSION.SDK_INT}) · ${android.os.Build.MANUFACTURER} ${android.os.Build.MODEL}",
            "Screen ${cfg.screenWidthDp}x${cfg.screenHeightDp} dp, density ${dm.density}, font scale ${cfg.fontScale}",
            "Locale ${java.util.Locale.getDefault()}, time zone ${java.util.TimeZone.getDefault().id}",
            "Battery saver: ${runCatching { (app.getSystemService(android.content.Context.POWER_SERVICE) as android.os.PowerManager).isPowerSaveMode }.getOrNull()}",
        ).joinToString("\n")
    }
}
