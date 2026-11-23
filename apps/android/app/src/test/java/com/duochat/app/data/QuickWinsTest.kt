package com.duochat.app.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** The rules live in the shared engine (and are tested there); here only the reading and writing of its JSON. */
class QuickWinsTest {
    @Test
    fun `pinned chats come first in pin order`() {
        val rows = listOf("a", "b", "c", "d")
        assertEquals(listOf("c", "a", "b", "d"), withPinsFirst(rows, listOf("c", "a")) { it })
        assertEquals(rows, withPinsFirst(rows, listOf("gone")) { it })
    }

    @Test
    fun `the checkup question has the names the engine expects`() {
        val o = org.json.JSONObject(checkupInput(true, 5L, false, true, false, true, 2, 99L))
        assertEquals(true, o.getBoolean("app_lock"))
        assertEquals(5L, o.getLong("last_backup_at"))
        assertEquals(2, o.getInt("stale_devices"))
        assertEquals(99L, o.getLong("now_ms"))
    }

    @Test
    fun `the hosts question flattens the network settings`() {
        val o = org.json.JSONObject(hostsInput(NetworkConfig(forceRelay = true, turn = listOf(TurnServer(listOf("turn:turn.example.org:3478"), "u", "c")))))
        assertEquals("AUTOMATIC", o.getString("mode"))
        assertEquals(true, o.getBoolean("force_relay"))
        assertEquals("turn:turn.example.org:3478", o.getJSONArray("turn").getString(0))
        assertTrue(o.getJSONArray("nostr").length() > 0)
    }

    @Test
    fun `the hosts question carries Private mode and the update check`() {
        val o = org.json.JSONObject(hostsInput(NetworkConfig(torMode = true), updateChecks = true))
        assertEquals(true, o.getBoolean("tor"))
        assertEquals(true, o.getBoolean("updates"))
        assertEquals(false, org.json.JSONObject(hostsInput(NetworkConfig())).getBoolean("updates"))
    }

    @Test
    fun `the IP is hidden only when something really hides it`() {
        val turn = listOf(TurnServer(listOf("turn:turn.example.org:3478"), "u", "c"))
        // The switch with no relay behind it hides nothing (builtinTurn off so a build's own relay does not count).
        assertEquals(false, hidesIp(NetworkConfig(forceRelay = true, builtinTurn = false)))
        assertEquals(true, hidesIp(NetworkConfig(forceRelay = true, turn = turn)))
        assertEquals(false, hidesIp(NetworkConfig(forceRelay = false, turn = turn)))
        assertEquals(true, hidesIp(NetworkConfig(torMode = true)))
    }

    @Test
    fun `the engine's answers are read back`() {
        val items = parseCheckup("""[{"id":"lock","label":"App lock","ok":false,"hint":"Turn on"}]""")
        assertEquals(CheckupItem("lock", "App lock", false, "Turn on"), items.single())
        assertEquals(HostRow("relay.damus.io", "why"), parseHosts("""[{"host":"relay.damus.io","why":"why"}]""").single())
        val s = parseStorage("""[{"id":"x","messages":2,"files":1,"bytes":1005,"file_bytes":1000,"file_ids":["m1"]}]""") { it.uppercase() }.single()
        assertEquals("X", s.name)
        assertEquals(listOf("m1"), s.fileIds)
        assertTrue(parseCheckup("not json").isEmpty())
    }

    @Test
    fun `time of day is shown as HH_MM`() {
        assertEquals("07:05", minutesLabel(7 * 60 + 5))
    }

    @Test
    fun `delivery states are put in words`() {
        assertEquals("Read", deliveryWords(DeliveryState.READ))
        assertEquals("Waiting for them to come online", deliveryWords(DeliveryState.QUEUED))
    }

    @Test
    fun `the health question has the names the engine expects and keeps unknowns null`() {
        val o = org.json.JSONObject(healthInput("online", true, null, false, true, false, 2))
        assertEquals("android", o.getString("platform"))
        assertEquals("online", o.getString("signaling"))
        assertTrue(o.isNull("notification_permission"))
        assertEquals(false, o.getBoolean("battery_unrestricted"))
        assertEquals(2, o.getInt("muted_chats"))
    }

    @Test
    fun `the engine's health answer is read back`() {
        val h = parseHealth("""{"items":[{"id":"battery","ok":false,"title":"Battery","hint":"h","fix":"battery_settings"}],"verdict":{"ok":false,"id":"battery","text":"t","fix":"battery_settings"}}""")!!
        assertEquals(HealthItem("battery", false, "Battery", "h", "battery_settings"), h.items.single())
        assertEquals("battery_settings", h.verdict!!.fix)
        assertEquals(null, parseHealth("[]"))
    }

    @Test
    fun `a chat kind and a version go to the engine as JSON strings`() {
        assertEquals("\"direct\"", jsonString("direct"))
        assertEquals(SecurityNote("Sealed", listOf("a", "b")), parseSecurityNote("""{"headline":"Sealed","lines":["a","b"]}"""))
        assertEquals(WhatsNew("0.1.0", emptyList()), parseWhatsNew("""{"version":"0.1.0","lines":[]}"""))
    }

    @Test
    fun `the devices question and the diagnostics report carry only counts`() {
        val d = DeviceInfo("i", "Phone", 1L, true, null, false)
        val o = org.json.JSONObject(devicesInput(listOf(d), 7L))
        assertEquals(7L, o.getLong("now_ms"))
        assertTrue(o.getJSONArray("devices").getJSONObject(0).isNull("last_seen"))
        assertEquals("line", parseDevicesLine("""{"line":"line","others":[]}"""))
        val a = org.json.JSONObject(diagnosticsAppJson("0.1.0", 34, "AUTOMATIC", 3, false))
        assertEquals(3, a.getInt("relay_count"))
        assertEquals(SelfTestRow("Identity", true, "ok"), parseSelfTest("""[{"name":"Identity","ok":true,"detail":"ok"}]""").single())
    }

    @Test
    fun `quiet hours wrap midnight and mutes expire`() {
        val q = QuietHoursPref(true, 22 * 60, 7 * 60)
        assertTrue(inQuietNow(q, 23 * 60))
        assertTrue(inQuietNow(q, 6 * 60))
        assertEquals(false, inQuietNow(q, 12 * 60))
        assertEquals(false, inQuietNow(q.copy(enabled = false), 23 * 60))
        assertEquals(1, activeMutes(mapOf("a" to 200L, "b" to 50L), 100L))
    }
}
