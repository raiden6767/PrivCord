package com.duochat.app.data

import org.json.JSONArray
import org.json.JSONObject

/*
 * The privacy checkup, the hosts list and the storage count are worked out once, in the shared engine (insights.rs and
 * core/prefs.rs), and come back as JSON. This file only reads that JSON and builds the question; no rules live here.
 */

/** Pinned chats first (in the order they were pinned), then everything else as given. */
fun <T> withPinsFirst(rows: List<T>, pinned: List<String>, id: (T) -> String): List<T> {
    val pins = pinned.mapNotNull { p -> rows.firstOrNull { id(it) == p } }
    return pins + rows.filter { id(it) !in pinned }
}

fun minutesLabel(m: Int): String = "%02d:%02d".format(m / 60, m % 60)

/** One internet host this phone may contact, and why. */
data class HostRow(val host: String, val why: String)

data class CheckupItem(val id: String, val label: String, val ok: Boolean, val hint: String)

data class ChatStorage(val id: String, val name: String, val messages: Int, val files: Int, val bytes: Long, val fileBytes: Long, val fileIds: List<String>)

private fun JSONArray.objects(): List<JSONObject> = (0 until length()).map { getJSONObject(it) }

fun parseCheckup(json: String): List<CheckupItem> = runCatching {
    JSONArray(json).objects().map { CheckupItem(it.getString("id"), it.getString("label"), it.getBoolean("ok"), it.getString("hint")) }
}.getOrDefault(emptyList())

fun parseHosts(json: String): List<HostRow> = runCatching {
    JSONArray(json).objects().map { HostRow(it.getString("host"), it.getString("why")) }
}.getOrDefault(emptyList())

fun parseStorage(json: String, nameOf: (String) -> String): List<ChatStorage> = runCatching {
    JSONArray(json).objects().map { o ->
        val files = o.optJSONArray("file_ids")?.let { a -> (0 until a.length()).map { a.getString(it) } } ?: emptyList()
        ChatStorage(o.getString("id"), nameOf(o.getString("id")), o.getInt("messages"), o.getInt("files"), o.getLong("bytes"), o.getLong("file_bytes"), files)
    }
}.getOrDefault(emptyList())

/** The question the engine answers for the privacy checkup. */
fun checkupInput(
    appLock: Boolean, lastBackupAt: Long, hideIp: Boolean, previewsOff: Boolean, groupOnly: Boolean, panicWipe: Boolean,
    staleDevices: Int, now: Long = System.currentTimeMillis(),
): String = JSONObject()
    .put("app_lock", appLock).put("last_backup_at", lastBackupAt).put("hide_ip", hideIp).put("previews_off", previewsOff)
    .put("group_only", groupOnly).put("panic_wipe", panicWipe).put("stale_devices", staleDevices).put("now_ms", now)
    .toString()

/** The question the engine answers for the hosts list: the network settings, flattened. */
fun hostsInput(n: NetworkConfig, updateChecks: Boolean = false): String = JSONObject()
    .put("mode", n.transportMode.name)
    .put("force_relay", n.forceRelay)
    .put("nostr", JSONArray(n.signaling.flatMap { if (it is SignalingConfig.Nostr) it.relays else emptyList() }))
    .put("relays", JSONArray(n.signaling.mapNotNull { (it as? SignalingConfig.Relay)?.url }))
    .put("stun", JSONArray(n.stun))
    .put("turn", JSONArray(n.turn.flatMap { it.urls }))
    .put("tor", n.torMode)
    .put("updates", updateChecks)
    .toString()

/**
 * Whether friends are kept from seeing this phone's address: a relay carries every link (which needs a TURN server to
 * carry them), or Private mode sends everything through Tor. The "hide my IP" switch with no relay behind it hides nothing.
 */
fun hidesIp(n: NetworkConfig): Boolean = (n.forceRelay && n.hasTurn) || n.torMode

/** A message's delivery state in words. */
fun deliveryWords(s: DeliveryState): String = when (s) {
    DeliveryState.QUEUED -> "Waiting for them to come online"
    DeliveryState.SENT -> "Sent"
    DeliveryState.DELIVERED -> "Delivered"
    DeliveryState.READ -> "Read"
    DeliveryState.FAILED -> "Could not be sent"
}

/** The sealed-chat explanation the engine gives for a chat kind ("direct", "group" or "channel"). */
data class SecurityNote(val headline: String, val lines: List<String>)

data class WhatsNew(val version: String, val lines: List<String>)

data class HealthItem(val id: String, val ok: Boolean, val title: String, val hint: String, val fix: String)

data class HealthVerdict(val ok: Boolean, val id: String, val text: String, val fix: String)

data class Health(val items: List<HealthItem>, val verdict: HealthVerdict?)

data class SelfTestRow(val name: String, val ok: Boolean, val detail: String)

private fun JSONObject.strings(key: String): List<String> =
    optJSONArray(key)?.let { a -> (0 until a.length()).map { a.getString(it) } } ?: emptyList()

/** The engine takes a chat kind or a version as a JSON-encoded string. */
fun jsonString(s: String): String = JSONObject.quote(s)

fun parseSecurityNote(json: String): SecurityNote? = runCatching {
    JSONObject(json).let { SecurityNote(it.getString("headline"), it.strings("lines")) }
}.getOrNull()

fun parseWhatsNew(json: String): WhatsNew? = runCatching {
    JSONObject(json).let { WhatsNew(it.getString("version"), it.strings("lines")) }
}.getOrNull()

/** The delivery-health question; a null fact means "this phone cannot tell". */
fun healthInput(
    signaling: String, notificationsEnabled: Boolean, notificationPermission: Boolean?, batteryUnrestricted: Boolean?,
    backgroundAllowed: Boolean?, quietHoursNow: Boolean, mutedChats: Int,
): String = JSONObject()
    .put("platform", "android").put("signaling", signaling).put("notifications_enabled", notificationsEnabled)
    .put("notification_permission", notificationPermission ?: JSONObject.NULL)
    .put("battery_unrestricted", batteryUnrestricted ?: JSONObject.NULL)
    .put("background_allowed", backgroundAllowed ?: JSONObject.NULL)
    .put("quiet_hours_now", quietHoursNow).put("muted_chats", mutedChats)
    .toString()

fun parseHealth(json: String): Health? = runCatching {
    val o = JSONObject(json)
    val items = o.getJSONArray("items").objects().map {
        HealthItem(it.getString("id"), it.getBoolean("ok"), it.getString("title"), it.optString("hint"), it.optString("fix", "none"))
    }
    val v = o.optJSONObject("verdict")?.let { HealthVerdict(it.getBoolean("ok"), it.optString("id"), it.getString("text"), it.optString("fix", "none")) }
    Health(items, v)
}.getOrNull()

/** The linked-devices question: last_seen is milliseconds or null. */
fun devicesInput(devices: List<DeviceInfo>, now: Long = System.currentTimeMillis()): String = JSONObject()
    .put(
        "devices",
        JSONArray(
            devices.map {
                JSONObject().put("name", it.name).put("last_seen", it.lastSeen ?: JSONObject.NULL)
                    .put("this_device", it.thisDevice).put("stale", it.stale)
            },
        ),
    )
    .put("now_ms", now)
    .toString()

/** The one-line summary of the linked devices (the engine's `line`), or null. */
fun parseDevicesLine(json: String): String? = runCatching { JSONObject(json).getString("line").takeIf { it.isNotBlank() } }.getOrNull()

/** What the diagnostics report may say about this install: counts and versions only. */
fun diagnosticsAppJson(version: String, sdk: Int, mode: String, relays: Int, tor: Boolean): String = JSONObject()
    .put("version", version).put("platform", "android").put("sdk", sdk).put("transport_mode", mode)
    .put("relay_count", relays).put("tor_mode", tor)
    .toString()

fun parseSelfTest(json: String): List<SelfTestRow> = runCatching {
    JSONArray(json).objects().map { SelfTestRow(it.getString("name"), it.getBoolean("ok"), it.optString("detail")) }
}.getOrDefault(emptyList())

/** How many mutes are still running at [now] (the engine counts delivery health from this). */
fun activeMutes(muteUntil: Map<String, Long>, now: Long = System.currentTimeMillis()): Int = muteUntil.values.count { it > now }

/** Is the current local minute inside quiet hours (they may wrap midnight)? Inputs only; the engine decides what it means. */
fun inQuietNow(q: QuietHoursPref, minuteOfDay: Int): Boolean = q.enabled &&
    (if (q.from <= q.to) minuteOfDay in q.from until q.to else minuteOfDay >= q.from || minuteOfDay < q.to)

/** One of the recipient's devices and what it did with a message. [state] is "delivered", "read" or "waiting". */
data class Receipt(val deviceId: String, val deviceName: String?, val state: String, val at: Long)

fun parseReceipts(json: String): List<Receipt> = runCatching {
    JSONArray(json).objects().map {
        Receipt(it.getString("device_id"), if (it.isNull("device_name")) null else it.optString("device_name").ifBlank { null }, it.getString("state"), it.optLong("at", 0L))
    }
}.getOrDefault(emptyList())

/** "Laptop: read 12:03"; a device that has not got it yet shows "waiting". */
fun receiptLine(r: Receipt, timeOf: (Long) -> String): String {
    val who = r.deviceName ?: ("Device " + r.deviceId.take(4))
    return if (r.state == "waiting" || r.at <= 0L) "$who: waiting" else "$who: ${r.state} ${timeOf(r.at)}"
}

/** Conversation id to the number of nearby phones its Bluetooth route passes through. */
fun parseMeshHops(json: String): Map<String, Int> = runCatching {
    val o = JSONObject(json)
    o.keys().asSequence().associateWith { o.getInt(it) }
}.getOrDefault(emptyMap())

/** The Bluetooth banner's route: straight to the phone, or through [hops] nearby phones. */
fun hopsWords(hops: Int?): String = when {
    hops == null || hops <= 0 -> "directly"
    hops == 1 -> "through 1 nearby phone"
    else -> "through $hops nearby phones"
}
