package com.duochat.app.data

import org.json.JSONArray
import org.json.JSONObject

/**
 * "Use my server": one address (and a token if the server asks for one). The relay, STUN and a short-lived TURN login come
 * from the server itself (GET /.well-known/privcord-server, GET /turn; see crates/privcord-server/src/net.rs). They are laid
 * over the saved network settings when the engine reads them and never written into them.
 */
data class SavedServer(val base: String, val token: String, val name: String) {
    fun relayUrl(): String {
        val ws = MyServer.wsBase(base)
        return if (token.isEmpty()) ws else "$ws/?token=" + java.net.URLEncoder.encode(token, "UTF-8")
    }
}

data class ServerTurn(val urls: List<String>, val username: String, val credential: String, val expiresAt: Long)

data class ServerInfo(
    val stun: List<String>,
    val turn: ServerTurn?,
    val checkedAt: Long,
    val error: String? = null,
)

class MyServerException(message: String, val kind: Kind) : Exception(message) {
    enum class Kind { UNREACHABLE, UNAUTHORIZED, NOT_PRIVCORD, NEEDS_TOKEN, INVALID }
}

/** (status, body) of a GET with an optional bearer token. Throws on a network failure. */
typealias Fetch = (url: String, token: String) -> Pair<Int, String>

object MyServer {
    const val MAX_SERVERS = 4
    const val REFRESH_BEFORE_MS = 15 * 60_000L
    const val RECHECK_MS = 6 * 3_600_000L

    fun candidates(input: String): List<String> {
        val text = input.trim()
        if (text.isEmpty() || text.length > 300 || text.any { it.isWhitespace() }) return emptyList()
        val m = Regex("^([a-zA-Z][a-zA-Z0-9+.-]*)://(.*)$").find(text)
        val scheme = m?.groupValues?.get(1)?.lowercase()
        val rest = m?.groupValues?.get(2) ?: text
        val host = rest.split('/', '?', '#').first()
        if (host.isEmpty() || host.contains('@')) return emptyList()
        return when (scheme) {
            null -> listOf("https://$host", "http://$host")
            "https", "wss" -> listOf("https://$host")
            "http", "ws" -> listOf("http://$host")
            else -> emptyList()
        }
    }

    /** Plain http is for this machine and private networks only: a token must never cross the internet unencrypted. */
    fun isPrivateHost(base: String): Boolean {
        val host = base.replaceFirst(Regex("^https?://", RegexOption.IGNORE_CASE), "").replace(Regex(":\\d+$"), "").trim('[', ']').lowercase()
        if (host == "localhost" || host.endsWith(".local") || host.endsWith(".localhost") || host == "::1") return true
        val p = Regex("^(\\d+)\\.(\\d+)\\.\\d+\\.\\d+$").find(host)?.groupValues ?: return false
        val a = p[1].toInt()
        val b = p[2].toInt()
        return a == 10 || a == 127 || (a == 192 && b == 168) || (a == 172 && b in 16..31) || (a == 100 && b in 64..127)
    }

    fun wsBase(base: String) = base.replaceFirst(Regex("^http", RegexOption.IGNORE_CASE), "ws")

    /** A plain `turn:` address is also the server's STUN address (the same port answers both). */
    fun stunFromTurn(urls: List<String>) =
        urls.filter { Regex("^turn:[^\\s?]+$", RegexOption.IGNORE_CASE).matches(it) }.map { "stun:" + it.substring(5) }

    private fun get(fetch: Fetch, url: String, token: String): JSONObject {
        val (status, body) = try {
            fetch(url, token)
        } catch (_: Exception) {
            throw MyServerException("Could not reach that address. Check it, and that the server is running.", MyServerException.Kind.UNREACHABLE)
        }
        if (status == 401) {
            throw if (token.isEmpty()) {
                MyServerException("This server needs an access token. Ask the person who runs it.", MyServerException.Kind.NEEDS_TOKEN)
            } else {
                MyServerException("The server did not accept the token.", MyServerException.Kind.UNAUTHORIZED)
            }
        }
        if (status !in 200..299) throw MyServerException("The server answered $status.", MyServerException.Kind.NOT_PRIVCORD)
        return try {
            JSONObject(body)
        } catch (_: Exception) {
            throw MyServerException("That address answers, but it does not look like a PrivCord server.", MyServerException.Kind.NOT_PRIVCORD)
        }
    }

    private fun strings(a: JSONArray?): List<String> =
        if (a == null) emptyList() else (0 until a.length()).mapNotNull { (a.opt(it) as? String)?.takeIf { s -> s.isNotEmpty() && s.length < 300 } }

    fun discover(input: String, token: String, fetch: Fetch, now: Long = System.currentTimeMillis()): Pair<SavedServer, ServerInfo> {
        // The app does not allow unencrypted traffic at all (usesCleartextTraffic is off), so only https is tried.
        val all = candidates(input)
        val tries = all.filter { it.startsWith("https://") }
        if (all.isNotEmpty() && tries.isEmpty()) {
            throw MyServerException("This app only connects to servers over https. Give the server a certificate (see its README) and use an https address.", MyServerException.Kind.INVALID)
        }
        if (tries.isEmpty()) throw MyServerException("Type the server’s address, for example relay.example.com.", MyServerException.Kind.INVALID)
        var last: MyServerException? = null
        for (base in tries) {
            try {
                val about = get(fetch, "$base/.well-known/privcord-server", token)
                if (about.optString("name") != "privcord-server") {
                    throw MyServerException("That address answers, but it is not a PrivCord server.", MyServerException.Kind.NOT_PRIVCORD)
                }
                if (about.optBoolean("requires_token", false) && token.isEmpty()) {
                    throw MyServerException("This server needs an access token. Ask the person who runs it.", MyServerException.Kind.NEEDS_TOKEN)
                }
                val turnAbout = about.optJSONObject("turn")
                val turn = turnAbout?.let {
                    val path = it.optString("credentials", "/turn").let { p -> if (p.startsWith("/")) p else "/turn" }
                    val t = get(fetch, "$base$path", token)
                    val urls = strings(t.optJSONArray("urls"))
                    val user = t.optString("username")
                    val cred = t.optString("credential")
                    if (urls.isEmpty() || user.isEmpty() || cred.isEmpty()) {
                        throw MyServerException("The server sent a TURN login that cannot be used.", MyServerException.Kind.INVALID)
                    }
                    val ttl = t.optLong("ttl", 3600L).takeIf { s -> s > 0 } ?: 3600L
                    ServerTurn(urls, user, cred, now + ttl * 1000)
                }
                val saved = SavedServer(base, token, base.replaceFirst(Regex("^https?://"), ""))
                return saved to ServerInfo(turn?.let { stunFromTurn(it.urls) } ?: emptyList(), turn, now)
            } catch (e: MyServerException) {
                last = e
                if (e.kind == MyServerException.Kind.UNAUTHORIZED || e.kind == MyServerException.Kind.NEEDS_TOKEN) break
            }
        }
        throw last ?: MyServerException("Could not reach that address.", MyServerException.Kind.UNREACHABLE)
    }

    /** Asks again; on a failure the old answer stays usable and carries the reason. */
    fun refresh(saved: SavedServer, previous: ServerInfo?, fetch: Fetch, now: Long = System.currentTimeMillis()): ServerInfo =
        try {
            discover(saved.base, saved.token, fetch, now).second
        } catch (e: MyServerException) {
            previous?.copy(error = e.message) ?: ServerInfo(emptyList(), null, now, e.message)
        }

    fun needsRefresh(info: ServerInfo?, now: Long = System.currentTimeMillis()): Boolean {
        if (info == null) return true
        if (info.turn != null && info.turn.expiresAt - now < REFRESH_BEFORE_MS) return true
        return now - info.checkedAt > (if (info.error != null) 60_000L else RECHECK_MS)
    }

    /** The saved settings with what the servers offer laid over them. */
    fun overlay(net: NetworkConfig, servers: List<Pair<SavedServer, ServerInfo?>>, now: Long = System.currentTimeMillis()): NetworkConfig {
        if (servers.isEmpty()) return net
        val stun = net.stun.toMutableList()
        val turn = net.turn.toMutableList()
        val signaling = net.signaling.toMutableList()
        for ((saved, info) in servers) {
            val relay = SignalingConfig.Relay(saved.relayUrl())
            if (relay !in signaling) signaling.add(relay)
            if (info == null) continue
            info.stun.forEach { if (it !in stun) stun.add(0, it) }
            val t = info.turn
            if (t != null && t.expiresAt > now) turn.add(TurnServer(t.urls, t.username, t.credential))
        }
        return net.copy(stun = stun, turn = turn, signaling = signaling)
    }

    fun toJson(servers: List<SavedServer>): JSONArray =
        JSONArray(servers.map { JSONObject().put("base", it.base).put("token", it.token).put("name", it.name) })

    fun fromJson(a: JSONArray?): List<SavedServer> {
        if (a == null) return emptyList()
        val out = mutableListOf<SavedServer>()
        for (i in 0 until a.length()) {
            val o = a.optJSONObject(i) ?: continue
            val base = candidates(o.optString("base")).firstOrNull() ?: continue
            if (out.any { it.base == base }) continue
            val name = o.optString("name").trim().take(80).ifEmpty { base.replaceFirst(Regex("^https?://"), "") }
            out.add(SavedServer(base, o.optString("token").take(512), name))
            if (out.size >= MAX_SERVERS) break
        }
        return out
    }

    /** A plain HTTP GET for the app (call off the main thread). */
    fun httpGet(url: String, token: String): Pair<Int, String> {
        val c = java.net.URL(url).openConnection() as java.net.HttpURLConnection
        try {
            c.connectTimeout = 8000
            c.readTimeout = 8000
            if (token.isNotEmpty()) c.setRequestProperty("Authorization", "Bearer $token")
            val status = c.responseCode
            val body = (if (status in 200..299) c.inputStream else c.errorStream)?.use { it.readBytes().take(65536).toByteArray().toString(Charsets.UTF_8) } ?: ""
            return status to body
        } finally {
            c.disconnect()
        }
    }
}

/** What each saved server last offered (in memory only: the TURN login lasts an hour). */
object MyServerState {
    private val info = java.util.concurrent.ConcurrentHashMap<String, ServerInfo>()
    fun get(base: String): ServerInfo? = info[base]
    fun set(base: String, value: ServerInfo) { info[base] = value }
}
