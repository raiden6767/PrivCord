package com.duochat.app.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class MyServerTest {
    // Taken from a running privcord-server.
    private val about = """{"name":"privcord-server","requires_token":true,"relay":{"path":"/"},"turn":{"urls":["turn:203.0.113.7:13578","turn:203.0.113.7:13579?transport=tcp"],"credentials":"/turn","ttl_secs":3600}}"""
    private val turn = """{"urls":["turn:203.0.113.7:13578","turn:203.0.113.7:13579?transport=tcp"],"username":"1795810950:48e2296c","credential":"c2VjcmV0","ttl":3600}"""

    private fun server(token: String = "tok"): Fetch = { url, given ->
        when {
            given != token -> 401 to "{}"
            url.endsWith("/.well-known/privcord-server") -> 200 to about
            url.endsWith("/turn") -> 200 to turn
            else -> 404 to "{}"
        }
    }

    @Test
    fun `candidates try https then http for a bare address`() {
        assertEquals(listOf("https://r.example.com", "http://r.example.com"), MyServer.candidates("r.example.com"))
        assertEquals(listOf("http://10.0.0.5:8787"), MyServer.candidates("http://10.0.0.5:8787/x"))
        assertEquals(listOf("https://r.example.com"), MyServer.candidates("wss://r.example.com/?a=1"))
        assertTrue(MyServer.candidates("ftp://x").isEmpty())
        assertTrue(MyServer.candidates("a b").isEmpty())
        assertTrue(MyServer.candidates("user@host").isEmpty())
    }

    @Test
    fun `plain http is never tried`() {
        var asked = false
        try {
            MyServer.discover("http://relay.example.com", "tok", { _, _ -> asked = true; 500 to "" })
            fail("expected an error")
        } catch (e: MyServerException) {
            assertEquals(MyServerException.Kind.INVALID, e.kind)
        }
        assertFalse(asked)
        assertTrue(MyServer.isPrivateHost("http://192.168.1.5:8787"))
        assertFalse(MyServer.isPrivateHost("http://8.8.8.8"))
        assertTrue(MyServer.isPrivateHost("http://localhost:8787"))
    }

    @Test
    fun `stun comes only from the plain turn address`() {
        assertEquals(listOf("stun:203.0.113.7:13578"), MyServer.stunFromTurn(listOf("turn:203.0.113.7:13578", "turn:203.0.113.7:13579?transport=tcp")))
    }

    @Test
    fun `finds the server and its login`() {
        val (saved, info) = MyServer.discover("r.example.com", "tok", server(), 1000)
        assertEquals("https://r.example.com", saved.base)
        assertEquals("1795810950:48e2296c", info.turn?.username)
        assertEquals(1000 + 3_600_000L, info.turn?.expiresAt)
        assertEquals(listOf("stun:203.0.113.7:13578"), info.stun)
    }

    @Test
    fun `says so for a missing or wrong token and an unreachable address`() {
        fun kind(token: String, fetch: Fetch): MyServerException.Kind = try {
            MyServer.discover("r.example.com", token, fetch)
            fail("expected an error"); throw IllegalStateException()
        } catch (e: MyServerException) {
            e.kind
        }
        assertEquals(MyServerException.Kind.NEEDS_TOKEN, kind("", server()))
        assertEquals(MyServerException.Kind.UNAUTHORIZED, kind("bad", server()))
        assertEquals(MyServerException.Kind.UNREACHABLE, kind("", { _, _ -> throw java.io.IOException("offline") }))
    }

    @Test
    fun `a failed refresh keeps the old login and says why`() {
        val (saved, info) = MyServer.discover("r.example.com", "tok", server(), 1000)
        val again = MyServer.refresh(saved, info, { _, _ -> throw java.io.IOException("offline") }, 2000)
        assertEquals(info.turn, again.turn)
        assertNotNull(again.error)
        assertFalse(MyServer.needsRefresh(info.copy(checkedAt = 0), 60_000))
        assertTrue(MyServer.needsRefresh(info, 3_600_000L - 60_000))
    }

    @Test
    fun `the server is laid over the settings and the saved settings stay as they were`() {
        val (saved, info) = MyServer.discover("r.example.com", "tok", server(), System.currentTimeMillis())
        val mine = NetworkConfig()
        val merged = MyServer.overlay(mine, listOf(saved to info))
        assertTrue(merged.signaling.contains(SignalingConfig.Relay(saved.relayUrl())))
        assertEquals(mine.turn.size + 1, merged.turn.size)
        assertEquals("stun:203.0.113.7:13578", merged.stun.first())
        assertEquals(NetworkConfig(), mine)
        assertEquals(mine, MyServer.overlay(mine, emptyList()))
    }

    @Test
    fun `saved servers survive a round trip and bad entries are dropped`() {
        val list = listOf(SavedServer("https://r.example.com", "t", "r.example.com"))
        assertEquals(list, MyServer.fromJson(MyServer.toJson(list)))
        assertTrue(MyServer.fromJson(null).isEmpty())
    }
}
