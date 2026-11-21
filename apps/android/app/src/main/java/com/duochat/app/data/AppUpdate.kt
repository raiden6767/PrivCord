package com.duochat.app.data

import org.json.JSONObject

/** What the release's `android.json` says about the newest build. */
data class UpdateInfo(val versionCode: Int, val versionName: String, val url: String, val sha256: String)

/** The pure parts of in-app updating (no network, no Android), so they can be tested. */
object AppUpdate {
    /** Updates come only from this project's `Android` release. */
    const val BASE = "https://github.com/raiden6767/PrivCord/releases/download/Android/"
    const val INFO_URL = BASE + "android.json"

    /** Reads `android.json`; null if it is malformed or points anywhere but our own release. */
    fun parse(json: String): UpdateInfo? = runCatching {
        val o = JSONObject(json)
        val info = UpdateInfo(o.getInt("versionCode"), o.getString("versionName"), o.getString("url"), o.getString("sha256").lowercase())
        info.takeIf { it.url.startsWith(BASE) && Regex("^[0-9a-f]{64}$").matches(it.sha256) }
    }.getOrNull()

    fun isNewer(info: UpdateInfo, currentVersionCode: Int) = info.versionCode > currentVersionCode

    /** The automatic check runs at most this often (the "Check now" button always asks). */
    const val RECHECK_MS = 30 * 60 * 1000L
    fun dueForCheck(lastCheckAt: Long, now: Long) = now - lastCheckAt >= RECHECK_MS

    /** A check contacts GitHub directly, so it is never made while Hide my IP or Tor mode is on. */
    fun mayCheck(network: NetworkConfig) = !network.forceRelay && !network.torMode
}
