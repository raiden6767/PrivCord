package com.duochat.app.engine

import android.os.SystemClock

/**
 * The chat engine as the screens see it, with every action a person takes written to the diagnostic log: what was asked,
 * how long it took and how it ended. Only safe particulars are logged (counts, switches, which kind), never message text,
 * names, codes or file contents. Reading state is not logged (it happens constantly); only actions are.
 */
class LoggedEngine(private val inner: ChatEngineApi) : ChatEngineApi by inner {

    private suspend inline fun <T> tracked(what: String, detail: String = "", block: () -> T): T {
        val t0 = SystemClock.elapsedRealtime()
        Diag.log("call", "$what: asked", detail)
        return try {
            val result = block()
            val ms = SystemClock.elapsedRealtime() - t0
            val failure = (result as? Result<*>)?.exceptionOrNull()
            if (failure != null) {
                Diag.log("error", "$what: failed", "${failure.javaClass.simpleName}: ${failure.message.orEmpty().take(120)} (${ms} ms)")
            } else {
                Diag.log("call", "$what: done", "${ms} ms" + if (ms > 2_000) " (slow)" else "")
            }
            result
        } catch (e: Throwable) {
            if (e is kotlinx.coroutines.CancellationException) {
                Diag.log("call", "$what: cancelled", "")
            } else {
                Diag.log("error", "$what: threw", "${e.javaClass.simpleName}: ${e.message.orEmpty().take(120)}")
            }
            throw e
        }
    }

    private fun note(what: String, detail: String = "") = Diag.log("call", what, detail)

    // Members that have a default body in the interface are NOT forwarded by `by inner`: without these two lines the call
    // would reach the interface's default (an error, and "load earlier" doing nothing). Nothing typed here is logged.
    override suspend fun recoverIdentity(phrase: String, passphrase: String) = tracked("create the identity from the recovery phrase") { inner.recoverIdentity(phrase, passphrase) }
    override suspend fun loadEarlier(conversationId: String) = tracked("load earlier messages") { inner.loadEarlier(conversationId) }
    override suspend fun addFriend(text: String) = tracked("add a friend (scan, paste or words)", "chars=${text.length}") { inner.addFriend(text) }
    override fun retryAnswer() { note("retry the answer to an invite"); inner.retryAnswer() }
    override fun clearAnswer() { note("clear the answer to an invite"); inner.clearAnswer() }
    override suspend fun deleteMessage(messageId: String, everyone: Boolean) = tracked("delete a message", "everyone=$everyone") { inner.deleteMessage(messageId, everyone) }
    override suspend fun react(messageId: String, emoji: String) = tracked("react to a message") { inner.react(messageId, emoji) }
    override suspend fun exportBackup(passphrase: String) = tracked("make a backup") { inner.exportBackup(passphrase) }
    override suspend fun importBackup(text: String, passphrase: String) = tracked("restore a backup", "chars=${text.length}") { inner.importBackup(text, passphrase) }
    override suspend fun joinChannel(raw: String, persist: Boolean) = tracked("join a channel", "saved=$persist") { inner.joinChannel(raw, persist) }
    override fun leaveChannel(conversationId: String) { note("leave a channel"); inner.leaveChannel(conversationId) }
    override fun muteSender(uid: String) { note("mute a sender"); inner.muteSender(uid) }
    override fun setMeshEnabled(on: Boolean) { note("Bluetooth mesh switch", "on=$on"); inner.setMeshEnabled(on) }
    override suspend fun createInvite() = tracked("make a six-word invite") { inner.createInvite() }
    override fun closeInvite() { note("close the invite"); inner.closeInvite() }
    override suspend fun acceptRequest(uid: String) = tracked("accept a friend request") { inner.acceptRequest(uid) }
    override fun dismissRequest(uid: String) { note("dismiss a friend request"); inner.dismissRequest(uid) }
    override suspend fun markChecked(uid: String) = tracked("mark the safety numbers as checked") { inner.markChecked(uid) }
    override suspend fun renameFriend(uid: String, name: String) = tracked("rename a friend") { inner.renameFriend(uid, name) }
    override suspend fun removeFriend(uid: String) = tracked("remove a friend") { inner.removeFriend(uid) }
    override fun setGroupOnly(value: Boolean) { note("group-only switch", "on=$value"); inner.setGroupOnly(value) }
    override suspend fun createGroup(name: String, memberUids: List<String>) = tracked("create a group", "members=${memberUids.size}") { inner.createGroup(name, memberUids) }
    override suspend fun addToGroup(groupId: String, uids: List<String>) = tracked("add to a group", "people=${uids.size}") { inner.addToGroup(groupId, uids) }
    override suspend fun removeFromGroup(groupId: String, uid: String) = tracked("remove from a group") { inner.removeFromGroup(groupId, uid) }
    override suspend fun leaveGroup(groupId: String) = tracked("leave a group") { inner.leaveGroup(groupId) }
    override suspend fun renameGroup(groupId: String, name: String) = tracked("rename a group") { inner.renameGroup(groupId, name) }
    override suspend fun startDeviceLink() = tracked("device link: show a code") { inner.startDeviceLink() }
    override suspend fun startScanLinkOffer(name: String) = tracked("device link: show a code to scan") { inner.startScanLinkOffer(name) }
    override suspend fun submitLinkCode(digits: String) = tracked("device link: confirm the digits") { inner.submitLinkCode(digits) }
    override suspend fun hostScannedLink(code: String) = tracked("device link: a code was scanned") { inner.hostScannedLink(code) }
    override suspend fun joinDeviceLink(code: String, name: String) = tracked("device link: join with a code") { inner.joinDeviceLink(code, name) }
    override suspend fun confirmDeviceLink(accept: Boolean) = tracked("device link: accept the new device", "accept=$accept") { inner.confirmDeviceLink(accept) }
    override fun cancelDeviceLink() { note("device link: cancelled"); inner.cancelDeviceLink() }
    override suspend fun togglePin(conversationId: String) = tracked("pin or unpin a chat") { inner.togglePin(conversationId) }
    override suspend fun setArchived(conversationId: String, archived: Boolean) = tracked("archive a chat", "archived=$archived") { inner.setArchived(conversationId, archived) }
    override suspend fun setMuteUntil(conversationId: String, untilMs: Long) = tracked("mute a chat") { inner.setMuteUntil(conversationId, untilMs) }
    override suspend fun setQuietHours(enabled: Boolean, from: Int, to: Int) = tracked("quiet hours", "on=$enabled") { inner.setQuietHours(enabled, from, to) }
    override suspend fun setSilent(conversationId: String, silent: Boolean) = tracked("silence a chat", "silent=$silent") { inner.setSilent(conversationId, silent) }
    override suspend fun setSecureScreen(on: Boolean) = tracked("screen privacy switch", "on=$on") { inner.setSecureScreen(on) }
    override suspend fun scheduleMessage(conversationId: String, text: String, at: Long) = tracked("schedule a message", "chars=${text.length}") { inner.scheduleMessage(conversationId, text, at) }
    override suspend fun cancelScheduled(id: String) = tracked("cancel a scheduled message") { inner.cancelScheduled(id) }
    override suspend fun deviceRemoval(action: com.duochat.app.data.DeviceRemoval) = tracked("device removal") { inner.deviceRemoval(action) }
    override suspend fun send(conversationId: String, rawText: String, replyToId: String?) = tracked("send a message", "chars=${rawText.length} reply=${replyToId != null}") { inner.send(conversationId, rawText, replyToId) }
    override suspend fun sendFile(conversationId: String, rawName: String, mime: String, bytes: ByteArray, replyToId: String?) =
        tracked("send a file", "kind=$mime bytes=${bytes.size}") { inner.sendFile(conversationId, rawName, mime, bytes, replyToId) }
    override suspend fun retry(messageId: String) = tracked("retry a message") { inner.retry(messageId) }
    override suspend fun clearChat(conversationId: String) = tracked("clear a chat") { inner.clearChat(conversationId) }
    override suspend fun clearLocalData() = tracked("erase everything") { inner.clearLocalData() }
    override fun reloadNetwork() { note("network settings changed"); inner.reloadNetwork() }
    override fun onNetworkAvailable() { note("the phone's network came back"); inner.onNetworkAvailable() }
    override suspend fun createManualOffer() = tracked("connect with a code: make an offer") { inner.createManualOffer() }
    override suspend fun applyManualAnswer(code: String) = tracked("connect with a code: apply the answer") { inner.applyManualAnswer(code) }
    override suspend fun acceptManualOffer(code: String) = tracked("connect with a code: accept an offer") { inner.acceptManualOffer(code) }
    override fun cancelManual() { note("connect with a code: cancelled"); inner.cancelManual() }
    override suspend fun connectBluetooth(uid: String, manual: Boolean) = tracked("connect over Bluetooth", "by hand=$manual") { inner.connectBluetooth(uid, manual) }
}
