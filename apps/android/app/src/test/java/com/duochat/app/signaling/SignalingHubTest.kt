package com.duochat.app.signaling

import com.duochat.app.security.SealedBlob
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Test

class SignalingHubTest {
    /** Hands out whatever the test pushes in, as a relay would. */
    private class Fake : SignalingTransport {
        override val name = "fake"
        var deliver: ((SealedBlob, String) -> Unit)? = null
        override fun start(mailbox: String, onBlob: (blob: SealedBlob, id: String) -> Unit) { deliver = onBlob }
        override suspend fun put(mailbox: String, blob: SealedBlob, kind: String, ttlSec: Int) = Unit
        override fun remove(mailbox: String, id: String) = Unit
        override fun status() = SignalingStatus.ONLINE
        override fun stop() = Unit
    }

    private fun envelope(iv: String) =
        // An anonymous envelope (version 3): no sender, no recipient, no signature.
        SealedBlob("", "", JSONObject(), iv, "c2VjcmV0IGJvZHk" + iv, "", 3, "kem", "key")

    @Test
    fun `every anonymous envelope gets through, and only a real copy is dropped`() {
        val t = Fake()
        val got = mutableListOf<String>()
        SignalingHub(listOf(t)).start("box") { blob, _, _ -> got.add(blob.iv) }
        t.deliver!!(envelope("one"), "e1")
        t.deliver!!(envelope("two"), "e2")
        t.deliver!!(envelope("three"), "e3")
        t.deliver!!(envelope("two"), "e2-again")
        assertEquals(listOf("one", "two", "three"), got)
    }
}
