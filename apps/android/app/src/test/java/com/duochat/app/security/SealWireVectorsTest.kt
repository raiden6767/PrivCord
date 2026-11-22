package com.duochat.app.security

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * docs/blob-wire-vectors.json is written by the Rust engine (crates/privcord-domain/tests/blob_wire_vectors.rs). Every
 * kind of blob it makes must get through this codec and come out unchanged, because the relay path decodes and
 * re-encodes each one before handing it to the engine.
 */
class SealWireVectorsTest {
    private val blobs = run {
        val file = listOf("../../../docs/blob-wire-vectors.json", "../../docs/blob-wire-vectors.json", "docs/blob-wire-vectors.json")
            .map(::File).first { it.exists() }
        val all = JSONObject(file.readText(Charsets.UTF_8)).getJSONArray("blobs")
        (0 until all.length()).map { all.getJSONObject(it) }
    }

    private fun json(wire: String) = JSONObject(String(SignatureCodec.base64UrlDecode(wire), Charsets.UTF_8))

    @Test fun thereAreVectorsForEveryKind() {
        assertTrue(blobs.map { it.getString("name") }.containsAll(listOf("link-request", "classic-v1", "hybrid-v2")))
    }

    @Test fun everyBlobDecodesAndComesBackUnchanged() {
        for (b in blobs) {
            val wire = b.getString("wire")
            val decoded = Seal.decode(wire)
            assertNotNull("${b.getString("name")} was refused by the Kotlin decoder", decoded)
            val again = json(Seal.encode(decoded!!))
            assertTrue("${b.getString("name")} changed on the way through: $again", json(wire).similar(again))
        }
    }

    @Test fun aHybridBlobWithoutItsKeysIsRefused() {
        val broken = json(blobs.first { it.getString("name") == "hybrid-v2" }.getString("wire")).apply { remove("kem") }
        assertEquals(null, Seal.decode(SignatureCodec.base64UrlEncode(broken.toString().toByteArray())))
    }
}
