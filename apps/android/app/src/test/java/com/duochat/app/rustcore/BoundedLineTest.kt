package com.duochat.app.rustcore

import java.io.BufferedReader
import java.io.StringReader
import org.junit.Test
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull

class BoundedLineTest {
    private fun reader(text: String) = BufferedReader(StringReader(text))

    @Test
    fun readsOrdinaryLinesAndStopsAtTheEnd() {
        val r = reader("one\r\ntwo\nlast")
        assertEquals("one", r.readLineMax(100))
        assertEquals("two", r.readLineMax(100))
        assertEquals("last", r.readLineMax(100))
        assertNull(r.readLineMax(100))
    }

    @Test
    fun aLineLongerThanTheLimitIsRefusedWithoutBeingReadToItsEnd() {
        val endless = "x".repeat(10_000) + "\n"
        val r = reader(endless)
        assertNull(r.readLineMax(4096))
        // it stopped near the limit: most of the text is still unread
        assertEquals(true, r.ready())
    }

    @Test
    fun aLineExactlyAtTheLimitIsAccepted() {
        assertEquals("x".repeat(8), reader("x".repeat(8) + "\n").readLineMax(8))
    }
}
