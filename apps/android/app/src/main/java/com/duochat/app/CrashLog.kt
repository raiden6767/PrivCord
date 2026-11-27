package com.duochat.app

import android.app.Application
import android.content.Context
import android.os.Build
import android.os.Process
import android.os.SystemClock
import java.io.File

/**
 * Keeps the last crash on the phone (nowhere else), so the next launch can recover from it and the user can copy
 * the details to whoever is fixing it. Only the stack trace and the phone model are written: no messages, no names.
 */
object CrashLog {
    private fun file(context: Context) = File(context.filesDir, "last-crash.txt")

    private val urlPattern = Regex("""\b[a-zA-Z][a-zA-Z0-9+.-]*://[^\s"'<>)\]]+""")
    private val ipPattern = Regex("""\b\d{1,3}(?:\.\d{1,3}){3}\b""")
    private val idPattern = Regex("""\b[0-9a-f]{32,}\b""")
    private val hostPattern = Regex("""\b[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:com|net|org|io|app|dev|lol|me|xyz|info|chat|social|cloud)\b""", RegexOption.IGNORE_CASE)
    private val classPattern = Regex("""\b(?:com|org|net|io|java|javax|kotlin|kotlinx|android|androidx|dalvik|libcore|uniffi)\.[A-Za-z0-9_.$]+""")

    /**
     * What a crash report may say: error messages can name the relay or server the app was talking to, an address, or a
     * key or id. Those are replaced; class names and the stack stay, because that is what the report is for.
     */
    fun scrub(text: String): String {
        // Class and package names look like host names (androidx.core, org.webrtc): keep them out of the host rule.
        val keep = mutableListOf<String>()
        val masked = classPattern.replace(text) { keep.add(it.value); "\u0000${keep.size - 1}\u0000" }
        val cleaned = hostPattern.replace(
            idPattern.replace(ipPattern.replace(urlPattern.replace(masked, "<url>"), "<address>"), "<id>"),
            "<host>",
        )
        return Regex("\u0000(\\d+)\u0000").replace(cleaned) { keep[it.groupValues[1].toInt()] }
    }

    /** A crash this soon after the app started is treated as a crash at start-up (one that could repeat on every launch). */
    private const val STARTUP_MS = 30_000L

    fun install(app: Application) {
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            record(app, error, thread.name)
            previous?.uncaughtException(thread, error)
        }
    }

    /**
     * Writes [error] as the last crash. With [fatal] false it is an error the app survived (the engine's background
     * work): shown as such, never treated as a crash at start-up, and never written over a report not yet seen.
     */
    fun record(context: Context, error: Throwable, where: String, fatal: Boolean = true) {
        // The diagnostic log gets a line too (the class and where, not the stack: that is in the crash report).
        runCatching { com.duochat.app.engine.Diag.log("error", if (fatal) "CRASH" else "error survived", "${error.javaClass.simpleName} in $where") }
        runCatching {
            if (!fatal && file(context).exists()) return
            val uptime = if (!fatal) -1L else if (Build.VERSION.SDK_INT >= 24) SystemClock.elapsedRealtime() - Process.getStartElapsedRealtime() else -1L
            file(context).writeText(
                "uptimeMs=$uptime${if (fatal) "" else ";survived"}\n" +
                    "PrivCord ${BuildConfig.VERSION_NAME} · Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT}) · " +
                    "${Build.MANUFACTURER} ${Build.MODEL} · $where\n\n" +
                    scrub(error.stackTraceToString()).take(24_000),
            )
        }
    }

    /**
     * A crash in native code (the Rust core, SQLCipher, WebRTC) never reaches the Java handler above. Android 11+ keeps
     * why the last run ended: if that was a crash we did not record, write it down from there (with the readable parts
     * of the native report: library and function names), so the next launch can still show it. Treated as a crash at
     * start-up, so the engine waits for the dialog. Call first thing in Application.onCreate.
     */
    fun captureLastExit(context: Context) {
        if (Build.VERSION.SDK_INT < 30) return
        runCatching {
            val am = context.getSystemService(android.app.ActivityManager::class.java) ?: return
            val info = am.getHistoricalProcessExitReasons(context.packageName, 0, 1).firstOrNull() ?: return
            val native = info.reason == android.app.ApplicationExitInfo.REASON_CRASH_NATIVE
            if (!native && info.reason != android.app.ApplicationExitInfo.REASON_CRASH) return
            val prefs = context.getSharedPreferences("crashlog", Context.MODE_PRIVATE)
            if (prefs.getLong("seenExit", 0L) >= info.timestamp) return
            // Android keeps exit records across updates: a crash of the version this one replaced is not ours to report.
            val installed = context.packageManager.getPackageInfo(context.packageName, 0).lastUpdateTime
            if (info.timestamp < installed) return
            prefs.edit().putLong("seenExit", info.timestamp).commit()
            // A Java crash was already written by the handler, with its full stack trace.
            if (!native && file(context).exists()) return
            val trace = info.traceInputStream?.use { it.readBytes().take(512_000).toByteArray() }
            val readable = trace?.let { bytes ->
                String(bytes, Charsets.ISO_8859_1).split(Regex("[^\\x20-\\x7e]+")).filter { it.length >= 6 }.joinToString("\n")
            }.orEmpty()
            file(context).writeText(
                "uptimeMs=0\n" +
                    "PrivCord ${BuildConfig.VERSION_NAME} · Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT}) · " +
                    "${Build.MANUFACTURER} ${Build.MODEL} · ${if (native) "native crash" else "crash"}\n\n" +
                    "${scrub(info.description.orEmpty())}\n\n" + scrub(readable).take(20_000),
            )
        }
    }

    /** What the last run crashed with, if it did. It stays until [clear] (the user has seen it), so a crash that repeats
     *  before the dialog is answered never loses the details. */
    class Report(val details: String, val radio: Boolean, val atStartup: Boolean, val survived: Boolean)

    fun take(context: Context): Report? {
        val f = file(context)
        if (!f.exists()) return null
        // Written by the version this update replaced: its bug may well be fixed here, so it is not shown as current.
        val installed = runCatching { context.packageManager.getPackageInfo(context.packageName, 0).lastUpdateTime }.getOrDefault(0L)
        if (f.lastModified() < installed) {
            f.delete()
            return null
        }
        val text = runCatching { f.readText() }.getOrNull()
        if (text.isNullOrBlank()) return null
        val first = text.lineSequence().firstOrNull().orEmpty()
        val survived = first.endsWith(";survived")
        val uptime = first.removePrefix("uptimeMs=").removeSuffix(";survived").toLongOrNull() ?: -1L
        val details = text.substringAfter('\n')
        val radio = !survived && (details.contains("bluetooth", ignoreCase = true) || details.contains("com.duochat.app.ble."))
        return Report(details, radio, atStartup = !survived && uptime in 0..STARTUP_MS, survived = survived)
    }

    fun clear(context: Context) {
        file(context).delete()
    }
}
