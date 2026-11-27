package com.duochat.app

import com.duochat.app.data.NetworkConfig
import com.duochat.app.data.TurnServer
import com.duochat.app.data.NetworkConfig.Companion.parseTurnUrls

object AppConfig {
    /**
     * The key "What's new" is filed under: it is NOT the version shown to the person (that is BuildConfig.VERSION_NAME).
     */
    const val APP_VERSION = "0.1.0"

    // Who made it and under what licence: the same as docs/about.json (a test checks they agree).
    const val CREATED_BY = "raiden6767"
    const val LICENSE_SHORT = "PC-NC (non-commercial)"

    val conversationId: String =
        BuildConfig.CONVERSATION_ID.ifBlank { "duo" }

    /**
     * Network defaults. A TURN server given at build time (local.properties
     * turn.url / turn.username / turn.credential) seeds the editable setting.
     */
    fun defaultNetworkConfig(): NetworkConfig {
        val urls = parseTurnUrls(BuildConfig.TURN_URL)
        val turn = if (
            urls.isNotEmpty() &&
            BuildConfig.TURN_USERNAME.isNotBlank() &&
            BuildConfig.TURN_CREDENTIAL.isNotBlank()
        ) {
            listOf(TurnServer(urls, BuildConfig.TURN_USERNAME, BuildConfig.TURN_CREDENTIAL))
        } else {
            emptyList()
        }
        return NetworkConfig(turn = turn)
    }
}
