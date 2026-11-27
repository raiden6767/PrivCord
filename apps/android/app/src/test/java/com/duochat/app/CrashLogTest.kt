package com.duochat.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** A crash report is copied and sent by hand: it must not name the relay the app was talking to, an address or an id. */
class CrashLogTest {
    @Test
    fun `hosts, urls, addresses and ids are replaced`() {
        val out = CrashLog.scrub(
            "java.net.UnknownHostException: Unable to resolve host relay.damus.io\n" +
                "Caused by: failed to connect to wss://my-relay.example.org:8787/path from 192.168.1.20\n" +
                "uid " + "ab".repeat(32),
        )
        assertFalse(out.contains("damus"))
        assertFalse(out.contains("example.org"))
        assertFalse(out.contains("192.168"))
        assertFalse(out.contains("abab"))
        assertTrue(out.contains("<host>") && out.contains("<url>") && out.contains("<address>") && out.contains("<id>"))
    }

    @Test
    fun `class names and the stack are kept`() {
        val trace = "java.lang.IllegalStateException: boom\n\tat com.duochat.app.engine.EngineHost.ensureStarted(EngineHost.kt:51)\n" +
            "\tat androidx.core.app.ActivityCompat.start(ActivityCompat.java:12)\n\tat org.webrtc.PeerConnection.close(PeerConnection.java:9)"
        assertEquals(trace, CrashLog.scrub(trace))
    }
}
