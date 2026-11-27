package com.duochat.app.ble

import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * What the Bluetooth radio did lately, in words, newest last: advertising started or refused (with Android's code), a
 * scan began, a friend's tag was seen, a connection changed state (with its status code). Shown in Settings so a failed
 * test on a real phone says what happened. Holds no names, addresses or keys.
 */
object BleLog {
    private const val KEEP = 14
    private val lines = ArrayDeque<String>()
    private val clock = SimpleDateFormat("HH:mm:ss", Locale.US)

    fun add(text: String) {
        android.util.Log.i("DuoChatBle", text)
        com.duochat.app.engine.Diag.log("ble", text)
        synchronized(lines) {
            lines.addLast(clock.format(Date()) + "  " + text)
            while (lines.size > KEEP) lines.removeFirst()
        }
    }

    fun recent(): List<String> = synchronized(lines) { lines.toList() }

    /** Android's advertising failure codes, in words. */
    fun advertiseFailure(code: Int): String = when (code) {
        1 -> "the advertisement is too large"
        2 -> "too many advertisers are already running"
        3 -> "already advertising"
        4 -> "an internal error"
        5 -> "this phone does not support advertising"
        else -> "code $code"
    }

    /** Android's scan failure codes, in words. */
    fun scanFailure(code: Int): String = when (code) {
        1 -> "a scan with these settings is already running"
        2 -> "the app could not be registered for scanning"
        3 -> "an internal error"
        4 -> "this phone does not support scanning"
        5 -> "scanning is out of hardware resources"
        6 -> "scanning too often: Android is holding it back"
        else -> "code $code"
    }
}
