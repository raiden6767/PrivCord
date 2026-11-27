package com.duochat.app.engine

import com.duochat.app.data.ActiveInvite
import com.duochat.app.data.ChatState
import com.duochat.app.data.FileMeta
import kotlinx.coroutines.flow.StateFlow

/**
 * Everything the screens need from a chat engine. Two implementations: [ChatEngine] (the original Kotlin one) and
 * `RustChatEngine` (the same behaviour from the shared Rust engine, in the "rust" build flavor).
 */
interface ChatEngineApi {
    val state: StateFlow<ChatState>

    suspend fun start()
    fun stop()
    fun setVisible(isVisible: Boolean)
    fun setOpenChat(conversationId: String?)
    fun clearNotice()
    fun onOwnNameChanged()
    fun ownCard(): String
    suspend fun addFriend(text: String): Result<String>
    fun retryAnswer()
    fun clearAnswer()
    suspend fun deleteMessage(messageId: String, everyone: Boolean)

    /** Load another page of a chat's older messages (the engine keeps only the newest ones on screen until asked). */
    suspend fun loadEarlier(conversationId: String) {}
    suspend fun react(messageId: String, emoji: String)
    fun search(query: String, limit: Int = 50): List<SearchHit>
    suspend fun exportBackup(passphrase: String): Result<String>
    suspend fun importBackup(text: String, passphrase: String): Result<String>
    suspend fun joinChannel(raw: String, persist: Boolean = true): Result<String>
    fun leaveChannel(conversationId: String)
    fun muteSender(uid: String)
    fun setMeshEnabled(on: Boolean)
    suspend fun createInvite(): Result<ActiveInvite>
    /** Stop publishing the six-word invite (the Add a friend screen was left). */
    fun closeInvite()
    suspend fun acceptRequest(uid: String): Result<String>
    fun dismissRequest(uid: String)
    suspend fun markChecked(uid: String)
    suspend fun renameFriend(uid: String, name: String)
    fun removeBlocker(uid: String): String?
    suspend fun removeFriend(uid: String): Result<Unit>
    fun setGroupOnly(value: Boolean)
    suspend fun createGroup(name: String, memberUids: List<String>): Result<String>
    suspend fun addToGroup(groupId: String, uids: List<String>): Result<Unit>
    suspend fun removeFromGroup(groupId: String, uid: String): Result<Unit>
    suspend fun leaveGroup(groupId: String): Result<Unit>

    /** Make a group from before the protocol update again (same name, its friends, its messages). Returns its new id. */
    suspend fun recreateLegacyGroup(oldId: String): Result<String>

    /** Stop offering a group from before the protocol update. */
    suspend fun dismissLegacyGroup(oldId: String): Result<Unit>
    suspend fun renameGroup(groupId: String, name: String): Result<Unit>

    /** On the device holding the account key: a code (shown as a QR code) for a new device. */
    suspend fun startDeviceLink(): Result<String>

    /** On a new device: a code to show as a QR for the other device to scan; that device then shows six digits. */
    suspend fun startScanLinkOffer(name: String): Result<String>

    /** On a new device: the six digits the other device shows. True when they match. */
    suspend fun submitLinkCode(digits: String): Result<Boolean>

    /** On the device holding the account key: the code scanned from the new device's QR (it then shows six digits). */
    suspend fun hostScannedLink(code: String): Result<Unit>

    /** On a new device: join the account with the code from the other device. */
    suspend fun joinDeviceLink(code: String, name: String): Result<Unit>

    /** On the device holding the account key: the 6 digits match (or not). */
    suspend fun confirmDeviceLink(accept: Boolean): Result<Unit>
    fun cancelDeviceLink()

    /** Pins, archive, mutes and quiet hours live in the shared engine and follow the person to every device. */
    suspend fun togglePin(conversationId: String): Result<Unit>
    suspend fun setArchived(conversationId: String, archived: Boolean): Result<Unit>
    /** Turn a chat's notifications off until a time in ms (0 turns them on again). */
    suspend fun setMuteUntil(conversationId: String, untilMs: Long): Result<Unit>
    suspend fun setQuietHours(enabled: Boolean, from: Int, to: Int): Result<Unit>
    /** Turn a chat's notification sound and vibration off or on. */
    suspend fun setSilent(conversationId: String, silent: Boolean): Result<Unit>
    /** Per-device delivery of one message, as JSON from the shared engine. */
    suspend fun messageReceiptsJson(messageId: String): String
    /** How many nearby phones each chat's Bluetooth route passes through, as JSON from the shared engine. */
    suspend fun meshHopsJson(): String
    /** Ready-made times for "send later" in the person's own time zone: label and time in ms. */
    suspend fun quickSendTimes(): List<Pair<String, Long>>

    /** Small pure answers worked out in the shared engine (`checkup`, `hosts`): JSON in, JSON out. */
    suspend fun insight(name: String, inputJson: String): String
    /** A counts-only report about this install (no messages, names or keys), as JSON from the shared engine. */
    suspend fun diagnosticsJson(appJson: String): String
    /** The engine's built-in self-test: a JSON list of {name, ok, detail}. */
    suspend fun selfTestJson(): String
    /** Hide the app from screenshots and the app switcher (kept in the shared preferences). */
    suspend fun setSecureScreen(on: Boolean)
    /** What each chat takes on this phone, as JSON from the shared engine. */
    suspend fun storageByChatJson(): String

    /** Scheduled send: a message to go out at [at] (milliseconds); kept on this phone, sent by it. */
    suspend fun scheduleMessage(conversationId: String, text: String, at: Long): Result<Unit>
    suspend fun cancelScheduled(id: String): Result<Unit>

    /** Removing another device of the account (see [com.duochat.app.data.DeviceRemoval]). */
    suspend fun deviceRemoval(action: com.duochat.app.data.DeviceRemoval): Result<Unit>
    suspend fun send(conversationId: String, rawText: String, replyToId: String? = null): Result<Unit>
    suspend fun sendFile(conversationId: String, rawName: String, mime: String, bytes: ByteArray, replyToId: String? = null): Result<Unit>
    suspend fun getFile(messageId: String): Pair<FileMeta, ByteArray>?
    suspend fun retry(messageId: String)
    suspend fun clearChat(conversationId: String)
    suspend fun clearLocalData()
    fun confirmManual()
    fun reloadNetwork()
    fun onNetworkAvailable()
    suspend fun createManualOffer(): String
    suspend fun applyManualAnswer(code: String)
    suspend fun acceptManualOffer(code: String): String
    fun cancelManual()
    fun refreshBluetooth()
    suspend fun connectBluetooth(uid: String, manual: Boolean = true): Result<Unit>
}

/** Something the user should hear about while the app is in the background. */
class IncomingNotice(val id: String, val conversationId: String, val title: String, val body: String, val isMessage: Boolean = true, val silent: Boolean = false)

/** One message found by [ChatEngineApi.search]. */
class SearchHit(val messageId: String, val conversationId: String, val conversationName: String, val senderName: String, val text: String, val at: Long)
