package com.duochat.app.rustcore

import android.content.Context
import android.net.wifi.WifiManager
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import uniffi.privcord_ffi.LanPeerDto
import java.io.BufferedReader
import java.io.BufferedWriter
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.ConcurrentHashMap

/**
 * The account's other devices on the local network, with no internet and no relay (docs/design/syncengine.md step 7).
 * The same as the desktop's `privcord_desktop_host::lan`: every few seconds the engine's beacon goes to the broadcast
 * address; a beacon only devices of the account can read names one of them and its port; the side the engine picks
 * connects over TCP, saying who it is with a fresh beacon of its own; then one frame per line, and the engine
 * authenticates and encrypts the link with both devices' own keys.
 */
class AndroidLan(
    private val app: Context,
    private val scope: CoroutineScope,
    private val beacon: (port: Int) -> String?,
    private val seen: (beacon: String) -> LanPeerDto?,
    private val onUp: (peer: String) -> Unit,
    private val onFrame: (peer: String, text: String) -> Unit,
    private val onDown: (peer: String) -> Unit,
) {
    private val writers = ConcurrentHashMap<String, BufferedWriter>()
    private val sockets = ConcurrentHashMap<String, Socket>()
    @Volatile private var running = false
    private var server: ServerSocket? = null
    private var udp: DatagramSocket? = null
    private var lock: WifiManager.MulticastLock? = null

    fun start() {
        if (running) return
        running = true
        val listener = runCatching { ServerSocket(0) }.getOrNull() ?: return
        server = listener
        val port = listener.localPort
        val socket = runCatching {
            DatagramSocket(null).apply {
                reuseAddress = true
                broadcast = true
                soTimeout = 1000
                bind(InetSocketAddress(BEACON_PORT))
            }
        }.getOrNull() ?: return
        udp = socket
        // Without this, many phones drop broadcast packets to save power.
        lock = runCatching {
            (app.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager)
                .createMulticastLock("duocord-lan").apply { setReferenceCounted(false); acquire() }
        }.getOrNull()
        // Beacons out.
        scope.launch(Dispatchers.IO) {
            val to = InetAddress.getByName("255.255.255.255")
            while (running && isActive) {
                beacon(port)?.toByteArray()?.let { b -> runCatching { socket.send(DatagramPacket(b, b.size, to, BEACON_PORT)) } }
                delay(EVERY_MS)
            }
        }
        // Beacons in: the side the engine picks connects.
        scope.launch(Dispatchers.IO) {
            val buf = ByteArray(2048)
            while (running && isActive) {
                val packet = DatagramPacket(buf, buf.size)
                if (runCatching { socket.receive(packet) }.isFailure) continue
                val text = String(packet.data, 0, packet.length, Charsets.UTF_8)
                val peer = seen(text) ?: continue
                if (!peer.initiator || writers.containsKey(peer.address)) continue
                val hello = beacon(port) ?: continue
                val s = runCatching { Socket().apply { connect(InetSocketAddress(packet.address, peer.port.toInt()), 3000) } }.getOrNull() ?: continue
                val w = BufferedWriter(OutputStreamWriter(s.getOutputStream(), Charsets.UTF_8))
                if (runCatching { w.write(hello); w.write("\n"); w.flush() }.isFailure) { runCatching { s.close() }; continue }
                open(peer.address, s, w)
            }
        }
        // Connections in: only another device of ours, saying so with a beacon of its own.
        scope.launch(Dispatchers.IO) {
            while (running && isActive) {
                val s = runCatching { listener.accept() }.getOrNull() ?: continue
                launch {
                    s.soTimeout = 5000
                    val r = BufferedReader(InputStreamReader(s.getInputStream(), Charsets.UTF_8))
                    val first = runCatching { r.readLineMax(MAX_HELLO) }.getOrNull()
                    val peer = first?.let { seen(it.trim()) }
                    if (peer == null || peer.initiator) { runCatching { s.close() }; return@launch }
                    s.soTimeout = 0
                    open(peer.address, s, BufferedWriter(OutputStreamWriter(s.getOutputStream(), Charsets.UTF_8)), r)
                }
            }
        }
    }

    private fun open(peer: String, s: Socket, w: BufferedWriter, reader: BufferedReader? = null) {
        sockets.put(peer, s)?.let { runCatching { it.close() } }
        writers[peer] = w
        onUp(peer)
        scope.launch(Dispatchers.IO) {
            val r = reader ?: BufferedReader(InputStreamReader(s.getInputStream(), Charsets.UTF_8))
            while (running) {
                val line = runCatching { r.readLineMax(MAX_LINE) }.getOrNull() ?: break
                onFrame(peer, line)
            }
            if (sockets.remove(peer, s)) {
                writers.remove(peer)
                runCatching { s.close() }
                onDown(peer)
            }
        }
    }

    fun isOpen(peer: String): Boolean = writers.containsKey(peer)

    /** One frame to a device we are connected to; false when there is no connection (or it broke). */
    fun send(peer: String, frame: String): Boolean {
        val w = writers[peer] ?: return false
        return runCatching { synchronized(w) { w.write(frame); w.write("\n"); w.flush() } }.isSuccess.also { ok ->
            if (!ok) close(peer)
        }
    }

    fun close(peer: String) {
        writers.remove(peer)
        sockets.remove(peer)?.let { runCatching { it.close() } }
    }

    fun stop() {
        running = false
        for (peer in sockets.keys.toList()) close(peer)
        runCatching { server?.close() }
        runCatching { udp?.close() }
        runCatching { lock?.release() }
    }

    private companion object {
        const val BEACON_PORT = 47_811
        const val EVERY_MS = 5_000L
        const val MAX_LINE = 512 * 1024

        /** The longest first line (a beacon) taken from a connection that has proved nothing yet. */
        const val MAX_HELLO = 4096
    }
}

/** A line of at most `max` characters, or null at the end of the stream or when it runs longer (never read to its end). */
internal fun BufferedReader.readLineMax(max: Int): String? {
    val sb = StringBuilder()
    while (true) {
        val c = read()
        if (c < 0) return if (sb.isEmpty()) null else sb.toString()
        if (c == '\n'.code) return sb.toString().trimEnd('\r')
        if (sb.length >= max) return null
        sb.append(c.toChar())
    }
}
