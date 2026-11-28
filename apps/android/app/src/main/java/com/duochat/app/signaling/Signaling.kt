package com.duochat.app.signaling

import okhttp3.OkHttpClient
import java.util.concurrent.TimeUnit

/** How the relays as a whole are doing. The logic that decides it is Rust (`SignalingClient`); this is only its name here. */
enum class SignalingStatus { IDLE, CONNECTING, ONLINE, ERROR }

/** Orbot's SOCKS proxy on this phone (its default port). */
const val TOR_SOCKS_PORT = 9050

/**
 * The OkHttp client the relay sockets use. It pings every 30 seconds and closes a socket that stops answering, so a dead
 * connection is found without any extra code. Through Tor, every connection goes to Orbot's SOCKS proxy, and OkHttp hands
 * it the host name unresolved, so the name lookup happens inside Tor too. If Orbot is not running the connection fails:
 * nothing is ever sent around Tor.
 */
fun newSignalingClient(tor: Boolean = false): OkHttpClient = OkHttpClient.Builder()
    .pingInterval(30, TimeUnit.SECONDS)
    .readTimeout(0, TimeUnit.MILLISECONDS)
    .apply {
        if (tor) {
            proxy(java.net.Proxy(java.net.Proxy.Type.SOCKS, java.net.InetSocketAddress("127.0.0.1", TOR_SOCKS_PORT)))
            // Circuits are slow to build; a relay that answers late is not a dead relay.
            connectTimeout(60, TimeUnit.SECONDS)
        }
    }
    .build()

/** Whether Orbot's SOCKS proxy answers on this phone right now (blocking; call off the main thread). */
fun torIsRunning(): Boolean = runCatching {
    java.net.Socket().use { it.connect(java.net.InetSocketAddress("127.0.0.1", TOR_SOCKS_PORT), 1_000) }
}.isSuccess
