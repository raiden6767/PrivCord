package com.duochat.app.rustcore

import com.duochat.app.data.NetworkConfig
import com.duochat.app.data.SignalingConfig
import com.duochat.app.signaling.SignalingStatus
import com.duochat.app.signaling.newSignalingClient
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import uniffi.privcord_ffi.RelayStatus
import uniffi.privcord_ffi.SignalingClient
import uniffi.privcord_ffi.SignalingNews
import uniffi.privcord_ffi.SignalingStep
import uniffi.privcord_ffi.SocketAction
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger

/**
 * The relay connections of this phone. **All the logic is Rust** (`SignalingClient`, the engine's `SignalingMux`, the same
 * code Windows runs): one connection per relay for every mailbox, subscriptions, catching up after a drop, reconnect
 * backoff, one copy of each blob, one answer per publish. This class only opens OkHttp WebSockets, tells Rust what happened
 * on them, and does what Rust asks (connect, send, close). OkHttp pings and closes a socket that stops answering by itself.
 */
class RelaySockets(
    private val scope: CoroutineScope,
    private val network: () -> NetworkConfig,
    /** A blob arrived for a mailbox (still encoded): hand it to the engine. */
    private val deliver: (mailbox: String, blob: String) -> Unit,
    /** The relays' state changed (online or not): refresh what the screen shows. */
    private val changed: () -> Unit,
) {
    private class Pending(val mailbox: String, val blob: String, val live: Boolean, val ttlSec: Int, val until: Long)

    @Volatile private var rust: SignalingClient? = null
    @Volatile private var http: OkHttpClient? = null
    private val sockets = ConcurrentHashMap<String, WebSocket>()

    /** Bumped each time a socket is opened or closed on purpose, so a late callback from an old socket is ignored. */
    private val generation = ConcurrentHashMap<String, AtomicInteger>()
    private val mailboxes = LinkedHashSet<String>()
    private val waiting = CopyOnWriteArrayList<Pending>()
    private val inFlight = ConcurrentHashMap<String, Pending>()
    private val putSeq = AtomicInteger()
    private var ticker: Job? = null
    @Volatile private var lastStatus = RelayStatus.IDLE
    @Volatile private var stopped = false

    private fun now() = System.currentTimeMillis()

    private fun configured(): Boolean = network().signaling.isNotEmpty()

    /** (Re)build the Rust client from the current settings and listen on every mailbox again. */
    @Synchronized
    private fun rebuild() {
        closeEverything()
        if (!configured()) return
        val cfg = network()
        val nostr = cfg.signaling.filterIsInstance<SignalingConfig.Nostr>().flatMap { it.relays }
        val relays = cfg.signaling.filterIsInstance<SignalingConfig.Relay>().map { it.url }
        http = newSignalingClient(cfg.torMode)
        val client = SignalingClient(nostr, relays, true)
        rust = client
        for (m in mailboxes.toList()) apply(client, client.subscribe(m, now()))
        startTicker()
    }

    private fun closeEverything() {
        ticker?.cancel()
        ticker = null
        val old = rust
        rust = null
        if (old != null) runCatching { apply(old, old.stop()) }
        for ((url, ws) in sockets.entries.toList()) {
            generation.getOrPut(url) { AtomicInteger() }.incrementAndGet()
            sockets.remove(url)
            runCatching { ws.cancel() }
        }
        runCatching { old?.close() }
    }

    private fun startTicker() {
        ticker?.cancel()
        ticker = scope.launch {
            while (isActive && !stopped) {
                delay(TICK_MS)
                val client = rust ?: break
                apply(client, client.tick(now()))
                flush(client)
                report(client)
            }
        }
    }

    @Synchronized
    fun subscribe(mailbox: String): Boolean {
        stopped = false
        if (!configured()) return false
        mailboxes.add(mailbox)
        val client = rust ?: run {
            rebuild()
            return rust != null
        }
        apply(client, client.subscribe(mailbox, now()))
        return true
    }

    @Synchronized
    fun unsubscribe(mailbox: String) {
        mailboxes.remove(mailbox)
        rust?.let { apply(it, it.unsubscribe(mailbox)) }
    }

    /** The settings changed (relays, Tor): start again on the new ones for every mailbox. */
    @Synchronized
    fun reconfigure() {
        if (stopped) return
        rebuild()
    }

    /** The phone moved to another network or woke up: every connection is suspect, so replace them all now. */
    @Synchronized
    fun networkChanged() {
        val client = rust ?: return
        apply(client, client.suspectAll(now()))
    }

    /** Publish. `false` when there is nothing to publish through (so the engine can hand it to mutual friends instead). */
    fun put(mailbox: String, blob: String, live: Boolean, ttlSec: Int): Boolean {
        val client = rust ?: return false
        if (client.status() == RelayStatus.ERROR || client.status() == RelayStatus.IDLE) return false
        waiting.add(Pending(mailbox, blob, live, ttlSec, now() + PUT_WAIT_MS))
        flush(client)
        return true
    }

    fun status(): SignalingStatus = when (rust?.status()) {
        RelayStatus.ONLINE -> SignalingStatus.ONLINE
        RelayStatus.CONNECTING -> SignalingStatus.CONNECTING
        RelayStatus.ERROR -> SignalingStatus.ERROR
        else -> SignalingStatus.IDLE
    }

    /** For "could not find that invite": how many relays answer right now. */
    fun describe(): String {
        val client = rust ?: return "no relay configured"
        val open = client.openLinks()
        val total = network().signaling.sumOf { if (it is SignalingConfig.Nostr) it.relays.size else 1 }
        return "$open of $total relays connected"
    }

    fun mailboxList(): List<String> = synchronized(this) { mailboxes.toList() }

    @Synchronized
    fun stop() {
        stopped = true
        closeEverything()
        mailboxes.clear()
        waiting.clear()
        inFlight.clear()
    }

    /** Sends what is waiting, once a relay is online. */
    private fun flush(client: SignalingClient) {
        if (client.status() != RelayStatus.ONLINE) {
            waiting.removeAll { it.until <= now() }
            return
        }
        for (p in waiting.toList()) {
            waiting.remove(p)
            val id = "a${putSeq.incrementAndGet()}"
            inFlight[id] = p
            apply(client, client.put(p.mailbox, p.blob, p.live, p.ttlSec.toUInt(), id, now()))
        }
    }

    private fun report(client: SignalingClient) {
        val s = client.status()
        if (s != lastStatus) {
            lastStatus = s
            changed()
        }
    }

    /** Does what Rust asked for and passes on what it reported, until nothing more comes back. */
    private fun apply(client: SignalingClient, first: SignalingStep) {
        val queue = ArrayDeque<SignalingStep>()
        queue.add(first)
        while (queue.isNotEmpty()) {
            val step = queue.removeFirst()
            for (n in step.news) {
                when (n) {
                    is SignalingNews.Blob -> {
                        deliver(n.mailbox, n.blob)
                        // A PrivCord relay may now forget it.
                        queue.add(client.remove(n.mailbox, n.id))
                    }
                    is SignalingNews.PutOk -> inFlight.remove(n.putId)
                    // A relay refused it, or none answered: it goes round again until its time is up.
                    is SignalingNews.PutFailed -> inFlight.remove(n.putId)?.let { if (now() < it.until) waiting.add(it) }
                }
            }
            for (a in step.actions) {
                when (a) {
                    is SocketAction.Connect -> connect(client, a.url, queue)
                    is SocketAction.Send -> {
                        val ok = sockets[a.url]?.send(a.text) == true
                        if (!ok) {
                            generation.getOrPut(a.url) { AtomicInteger() }.incrementAndGet()
                            sockets.remove(a.url)?.cancel()
                            queue.add(client.socketClosed(a.url, now()))
                        }
                    }
                    is SocketAction.Close -> {
                        generation.getOrPut(a.url) { AtomicInteger() }.incrementAndGet()
                        sockets.remove(a.url)?.close(1000, null)
                    }
                    // OkHttp pings by itself (and closes the socket when no pong comes back).
                    is SocketAction.Ping -> Unit
                }
            }
        }
    }

    private fun connect(client: SignalingClient, url: String, queue: ArrayDeque<SignalingStep>) {
        val request = try {
            Request.Builder().url(url).build()
        } catch (_: Exception) {
            queue.add(client.socketClosed(url, now()))
            return
        }
        val gen = generation.getOrPut(url) { AtomicInteger() }.incrementAndGet()
        val current = { generation[url]?.get() == gen && rust === client }
        val ws = (http ?: return).newWebSocket(
            request,
            object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) {
                    if (!current()) return
                    // Ours before anything is sent on it (this can run before `newWebSocket` has returned).
                    sockets[url] = webSocket
                    apply(client, client.socketOpened(url, now()))
                }

                override fun onMessage(webSocket: WebSocket, text: String) {
                    if (current()) apply(client, client.socketMessage(url, text, now()))
                }

                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = ended(webSocket)

                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) = ended(webSocket)

                private fun ended(webSocket: WebSocket) {
                    if (!current()) return
                    sockets.remove(url, webSocket)
                    apply(client, client.socketClosed(url, now()))
                }
            },
        )
        sockets[url] = ws
    }

    private companion object {
        const val TICK_MS = 500L
        const val PUT_WAIT_MS = 30_000L
    }
}
