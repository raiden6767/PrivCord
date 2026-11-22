package com.duochat.app.security

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

/** Device-linking blobs have an empty key; the relay path must still deliver them to the engine. */
class SealLinkBlobTest {
    private fun wire(to: String) = Seal.encode(
        SealedBlob(to, "link-request", JSONObject("""{"kty":"EC","crv":"P-256","x":"","y":""}"""), "", "ct", "sig"),
    )

    @Test fun linkBlobWithEmptyKeyDecodes() {
        val blob = Seal.decode(wire("link"))
        assertNotNull(blob)
        assertEquals("link-request", blob!!.from)
    }

    @Test fun otherBlobWithEmptyKeyIsStillRefused() {
        assertNull(Seal.decode(wire("someone")))
    }
}
