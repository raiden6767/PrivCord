package com.duochat.app.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AppUpdateTest {
    private val sha = "a".repeat(64)
    private fun json(url: String = AppUpdate.BASE + "PrivCord.apk", sha: String = this.sha) =
        """{"versionCode":7,"versionName":"0.1.7","url":"$url","sha256":"$sha"}"""

    @Test
    fun `reads a good android json`() {
        val info = AppUpdate.parse(json())!!
        assertEquals(7, info.versionCode)
        assertEquals("0.1.7", info.versionName)
        assertEquals(sha, info.sha256)
    }

    @Test
    fun `refuses a file that points anywhere but our own release, or has a bad checksum`() {
        assertNull(AppUpdate.parse(json(url = "https://example.com/PrivCord.apk")))
        assertNull(AppUpdate.parse(json(url = "http://github.com/raiden6767/PrivCord/releases/download/Android/PrivCord.apk")))
        assertNull(AppUpdate.parse(json(sha = "not-a-checksum")))
        assertNull(AppUpdate.parse("{}"))
        assertNull(AppUpdate.parse("not json"))
    }

    @Test
    fun `only a higher version code is an update`() {
        val info = AppUpdate.parse(json())!!
        assertTrue(AppUpdate.isNewer(info, 6))
        assertFalse(AppUpdate.isNewer(info, 7))
        assertFalse(AppUpdate.isNewer(info, 8))
    }

    @Test
    fun `no check while hide my ip or tor mode is on`() {
        assertTrue(AppUpdate.mayCheck(NetworkConfig()))
        assertFalse(AppUpdate.mayCheck(NetworkConfig(forceRelay = true)))
        assertFalse(AppUpdate.mayCheck(NetworkConfig(torMode = true)))
    }

    @Test
    fun `the automatic check repeats at most every half hour`() {
        val t = 10_000_000L
        assertTrue(AppUpdate.dueForCheck(0L, t))
        assertFalse(AppUpdate.dueForCheck(t, t + AppUpdate.RECHECK_MS - 1))
        assertTrue(AppUpdate.dueForCheck(t, t + AppUpdate.RECHECK_MS))
    }
}
