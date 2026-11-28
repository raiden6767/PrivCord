package com.duochat.app.data

import android.content.Context
import com.duochat.app.AppConfig
import com.duochat.app.security.LocalCrypto
import org.json.JSONArray
import org.json.JSONObject
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

data class AppSettings(
    val theme: String = "system",
    val accent: String = ACCENTS.first().second,
    /** A built-in colour theme (see Themes.kt), or "classic" for the system light/dark look with the accent picker. */
    val palette: String = CLASSIC_PALETTE,
    val notificationsEnabled: Boolean = true,
    // Off unless the person turns it on: message text on a lock screen, and in the system's notification history, is a leak.
    val notificationPreviews: Boolean = false,
    /** Local nickname the single paired friend had before multi-contact; used once to migrate it. */
    val peerNickname: String = "",
    /** The name friends see on your invite and in groups. */
    val displayName: String = "",
    /**
     * Privacy: stay group-only with people you only meet through a group. You still get their group messages
     * and they get yours, but neither of you can start a direct chat unless you swap invites yourselves.
     */
    val groupOnly: Boolean = false,
    /** Relay messages for nearby phones over Bluetooth (multi-hop mesh). Off by default: it keeps Bluetooth busy. */
    val mesh: Boolean = false,
    /** Keep PrivCord running in the background (shows a small permanent notification) so messages arrive with the app closed. */
    val keepRunning: Boolean = false,
    /** Panic wipe: tapping the app title three times quickly erases everything on this phone, with no questions. */
    val panicWipe: Boolean = false,
    /** Ask for fingerprint, face or the screen-lock PIN/pattern/password to open the app (when the phone has one). */
    val appLock: Boolean = true,
    /** Old per-phone mutes. They are moved into the shared engine once (muted with no end) and then cleared. */
    val mutedChats: Set<String> = emptySet(),
    /** When a backup was last made (milliseconds), or 0. Only used by the privacy checkup. */
    val lastBackupAt: Long = 0L,
    /** The app version whose "What's new" was last shown (empty before the first run). */
    val lastSeenVersion: String = "",
    /** Look for a new version when the app starts (contacts GitHub; never done while Hide my IP or Tor is on). */
    val updateChecks: Boolean = true,
    /** "Later" on the update prompt: do not ask again before this time (milliseconds). */
    val updateSnoozedUntil: Long = 0L,
    /** The build that "Later" put off: a newer build than this is offered at once, whatever the snooze. */
    val updateSnoozedFor: Int = 0,
    /** Show the advanced options in Settings (beginners see the short list). */
    val advanced: Boolean = false,
    val network: NetworkConfig = NetworkConfig(),
    /** PrivCord servers added with "Use my server" (address and token; what they offer is asked for again at each start). */
    val servers: List<SavedServer> = emptyList(),
)

val ACCENTS: List<Pair<String, String>> = listOf(
    "Plum" to "#7A3E73",
    "Pine" to "#2F6B4F",
    "Clay" to "#B4532A",
    "Rose" to "#B83A5E",
    "Slate" to "#3E4C5E",
)

class SettingsRepository(context: Context) {
    private val appContext = context.applicationContext
    private val prefs = appContext.getSharedPreferences("duochat.settings.v1", Context.MODE_PRIVATE)

    /**
     * The settings (your name, the relay and TURN settings with their password, the app-lock and panic-wipe switches) are
     * kept as one value encrypted by the Android Keystore. If the Keystore cannot be used they are not kept at all: they
     * are never written as plain text.
     */
    private val crypto = runCatching { LocalCrypto(appContext, "settings") }.getOrNull()

    // One copy for the whole process: the screen and the background service read and change the same settings.
    private val _settings = synchronized(SettingsRepository) { shared ?: MutableStateFlow(load()).also { shared = it } }
    val settings: StateFlow<AppSettings> = _settings.asStateFlow()

    fun update(patch: AppSettings.() -> AppSettings) {
        val next = _settings.value.patch()
        _settings.update { next }
        save(next)
    }

    private fun save(s: AppSettings) {
        val sealed = runCatching { crypto?.encrypt(toJson(s).toString()) }.getOrNull() ?: return
        // The older plain-text keys go in the same write that stores the encrypted copy.
        prefs.edit().clear().putString(SEALED, sealed).apply()
    }

    private fun toJson(s: AppSettings): JSONObject = JSONObject()
        .put("theme", s.theme).put("accent", s.accent).put("palette", s.palette)
        .put("notificationsEnabled", s.notificationsEnabled).put("notificationPreviews", s.notificationPreviews)
        .put("peerNickname", s.peerNickname).put("displayName", s.displayName)
        .put("groupOnly", s.groupOnly).put("mesh", s.mesh).put("keepRunning", s.keepRunning)
        .put("panicWipe", s.panicWipe).put("appLock", s.appLock)
        .put("mutedChats", JSONArray(s.mutedChats.toList()))
        .put("lastBackupAt", s.lastBackupAt).put("lastSeenVersion", s.lastSeenVersion)
        .put("updateChecks", s.updateChecks).put("updateSnoozedUntil", s.updateSnoozedUntil).put("updateSnoozedFor", s.updateSnoozedFor).put("advanced", s.advanced)
        .put("network", s.network.toJson())
        .put("servers", MyServer.toJson(s.servers))

    private fun load(): AppSettings {
        val sealed = prefs.getString(SEALED, null)
        if (sealed != null) {
            val opened = runCatching { crypto?.decrypt(sealed) }.getOrNull()
            val parsed = opened?.let { runCatching { fromJson(JSONObject(it)) }.getOrNull() }
            return parsed ?: AppSettings(network = AppConfig.defaultNetworkConfig())
        }
        // An older version kept each setting as plain text: read them once and keep them encrypted from now on.
        val legacy = loadLegacy()
        save(legacy)
        return legacy
    }

    private fun fromJson(o: JSONObject): AppSettings {
        val theme = o.optString("theme", "system")
        val accent = o.optString("accent", ACCENTS.first().second)
        return AppSettings(
            theme = if (theme in setOf("system", "dark", "light")) theme else "system",
            accent = if (Regex("^#[0-9a-fA-F]{6}$").matches(accent)) accent else ACCENTS.first().second,
            palette = o.optString("palette", CLASSIC_PALETTE).let { if (it == CLASSIC_PALETTE || themeById(it) != null) it else CLASSIC_PALETTE },
            notificationsEnabled = o.optBoolean("notificationsEnabled", true),
            notificationPreviews = o.optBoolean("notificationPreviews", false),
            peerNickname = o.optString("peerNickname", "").trim().take(40),
            displayName = o.optString("displayName", "").trim().take(40),
            groupOnly = o.optBoolean("groupOnly", false),
            mesh = o.optBoolean("mesh", false),
            keepRunning = o.optBoolean("keepRunning", false),
            panicWipe = o.optBoolean("panicWipe", false),
            appLock = o.optBoolean("appLock", true),
            mutedChats = o.optJSONArray("mutedChats")?.let { a -> (0 until a.length()).map { a.getString(it) }.toSet() } ?: emptySet(),
            lastBackupAt = o.optLong("lastBackupAt", 0L),
            lastSeenVersion = o.optString("lastSeenVersion", ""),
            updateChecks = o.optBoolean("updateChecks", true),
            updateSnoozedUntil = o.optLong("updateSnoozedUntil", 0L),
            updateSnoozedFor = o.optInt("updateSnoozedFor", 0),
            advanced = o.optBoolean("advanced", false),
            network = NetworkConfig.fromJson(o.optString("network", "").ifEmpty { null }, fallback = AppConfig.defaultNetworkConfig()),
            servers = MyServer.fromJson(o.optJSONArray("servers")),
        )
    }

    private fun loadLegacy(): AppSettings {
        val theme = prefs.getString("theme", "system") ?: "system"
        val accent = prefs.getString("accent", ACCENTS.first().second) ?: ACCENTS.first().second
        return AppSettings(
            theme = if (theme in setOf("system", "dark", "light")) theme else "system",
            accent = if (Regex("^#[0-9a-fA-F]{6}$").matches(accent)) accent else ACCENTS.first().second,
            palette = (prefs.getString("palette", CLASSIC_PALETTE) ?: CLASSIC_PALETTE).let { if (it == CLASSIC_PALETTE || themeById(it) != null) it else CLASSIC_PALETTE },
            notificationsEnabled = prefs.getBoolean("notificationsEnabled", true),
            notificationPreviews = prefs.getBoolean("notificationPreviews", true),
            peerNickname = (prefs.getString("peerNickname", "") ?: "").trim().take(40),
            displayName = (prefs.getString("displayName", "") ?: "").trim().take(40),
            groupOnly = prefs.getBoolean("groupOnly", false),
            mesh = prefs.getBoolean("mesh", false),
            keepRunning = prefs.getBoolean("keepRunning", false),
            panicWipe = prefs.getBoolean("panicWipe", false),
            appLock = prefs.getBoolean("appLock", true),
            mutedChats = prefs.getStringSet("mutedChats", emptySet())?.toSet() ?: emptySet(),
            lastBackupAt = prefs.getLong("lastBackupAt", 0L),
            lastSeenVersion = prefs.getString("lastSeenVersion", "") ?: "",
            network = NetworkConfig.fromJson(
                prefs.getString("network", null),
                fallback = AppConfig.defaultNetworkConfig(),
            ),
        )
    }

    private companion object {
        const val SEALED = "sealed"
        @Volatile var shared: MutableStateFlow<AppSettings>? = null
    }

}
