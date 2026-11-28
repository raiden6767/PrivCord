package com.duochat.app

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.unit.dp
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.duochat.app.data.EnginePhase
import com.duochat.app.ui.AddFriendScreen
import com.duochat.app.ui.AppViewModel
import com.duochat.app.ui.ChatScreen
import com.duochat.app.ui.DuoChatTheme
import com.duochat.app.ui.FatalScreen
import com.duochat.app.ui.FriendDetailsDialog
import com.duochat.app.ui.GroupDetailsScreen
import com.duochat.app.ui.HomeScreen
import com.duochat.app.ui.LoadingScreen
import com.duochat.app.ui.routeDepth
import com.duochat.app.ui.screenTransition
import com.duochat.app.ui.ManualConnectDialog
import com.duochat.app.ui.NewGroupScreen
import com.duochat.app.ui.NukeDialog
import com.duochat.app.ui.OnboardingScreen
import com.duochat.app.ui.SettingsDialog

class MainActivity : FragmentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            val viewModel: AppViewModel = viewModel()
            val settings by viewModel.settings.collectAsStateWithLifecycle()
            DuoChatTheme(settings) {
                Surface(modifier = Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
                    RootScreen(viewModel)
                }
            }
        }
    }

    // On screen or not is one of the inputs of the shared background policy (how hard Bluetooth scans, for one).
    override fun onStart() {
        super.onStart()
        com.duochat.app.engine.BackgroundPolicy.setForeground(true)
    }

    override fun onResume() {
        super.onResume()
        com.duochat.app.UpdateSignal.resumes.value++
    }

    override fun onStop() {
        com.duochat.app.engine.BackgroundPolicy.setForeground(false)
        super.onStop()
    }
}

// Where the user is, as plain text so it survives process death and rotation:
// "home", "add", "newgroup", "chat:<id>" (friend or group chat), "group:<groupId>" (details).
private const val HOME = "home"

@Composable
private fun RootScreen(viewModel: AppViewModel) {
    val chat by viewModel.chat.collectAsStateWithLifecycle()
    val settings by viewModel.settings.collectAsStateWithLifecycle()
    val killSwitchReport by viewModel.killSwitchReport.collectAsStateWithLifecycle()
    val locked by viewModel.locked.collectAsStateWithLifecycle()
    val context = LocalContext.current

    var route by rememberSaveable { mutableStateOf(HOME) }
    var settingsOpen by rememberSaveable { mutableStateOf(false) }
    var manualOpen by rememberSaveable { mutableStateOf(false) }
    var nukeOpen by rememberSaveable { mutableStateOf(false) }
    var panic by remember { mutableStateOf(false) }
    var channelsOpen by rememberSaveable { mutableStateOf(false) }
    var friendDialog by rememberSaveable { mutableStateOf<String?>(null) }

    val lifecycleOwner = LocalLifecycleOwner.current
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> { viewModel.onEntered(); viewModel.setVisible(true) }
                Lifecycle.Event.ON_STOP -> { viewModel.onLeft(); viewModel.setVisible(false) }
                else -> Unit
            }
            com.duochat.app.engine.Diag.log("app", "screen lifecycle", event.name.removePrefix("ON_").lowercase())
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }

    // Where the person is, for the diagnostic log (which chat is never named, only that one is open).
    LaunchedEffect(route) { com.duochat.app.engine.Diag.log("nav", "screen", route.substringBefore(':')) }
    LaunchedEffect(settingsOpen) { com.duochat.app.engine.Diag.log("nav", "settings", if (settingsOpen) "opened" else "closed") }
    LaunchedEffect(manualOpen) { com.duochat.app.engine.Diag.log("nav", "connect-with-a-code dialog", if (manualOpen) "opened" else "closed") }
    LaunchedEffect(channelsOpen) { com.duochat.app.engine.Diag.log("nav", "channels dialog", if (channelsOpen) "opened" else "closed") }
    LaunchedEffect(friendDialog != null) { com.duochat.app.engine.Diag.log("nav", "friend details dialog", if (friendDialog != null) "opened" else "closed") }
    LaunchedEffect(nukeOpen) { com.duochat.app.engine.Diag.log("nav", "erase-everything dialog", if (nukeOpen) "opened" else "closed") }

    val notificationPermission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        com.duochat.app.engine.Diag.log("perm", "notification permission", "granted=$granted")
        if (granted) viewModel.updateSettings { copy(notificationsEnabled = true) }
    }
    val requestNotifications = {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else {
            viewModel.updateSettings { copy(notificationsEnabled = true) }
        }
    }

    val state = chat
    // Screen privacy: hide this app from screenshots and the app switcher while the shared preference is on.
    val secureScreen = state?.prefs?.secureScreen == true
    LaunchedEffect(secureScreen) {
        (context as? android.app.Activity)?.window?.let {
            if (secureScreen) it.addFlags(android.view.WindowManager.LayoutParams.FLAG_SECURE) else it.clearFlags(android.view.WindowManager.LayoutParams.FLAG_SECURE)
        }
    }
    // "What's new", once after an update (the engine words it; the last version seen is kept on this phone).
    val newVersion = settings.lastSeenVersion != AppConfig.APP_VERSION
    val whatsNew by androidx.compose.runtime.produceState<com.duochat.app.data.WhatsNew?>(null, newVersion, state?.phase) {
        value = if (newVersion && state?.phase == EnginePhase.READY) {
            com.duochat.app.data.parseWhatsNew(viewModel.insight("whats_new", com.duochat.app.data.jsonString(AppConfig.APP_VERSION)))
        } else null
    }
    // A newer build, looked for when the app starts and again each time it comes back to the front (the process often stays
    // alive in the background, so "opening" the app is only a resume), at most every six hours. Never while Hide my IP or Tor
    // is on; "Later" snoozes it for 3 days.
    var update by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf<com.duochat.app.data.UpdateInfo?>(null) }
    val resumes = com.duochat.app.UpdateSignal.resumes.value
    LaunchedEffect(settings.updateChecks, settings.network.forceRelay, settings.network.torMode, resumes) {
        val now = System.currentTimeMillis()
        if (update == null && settings.updateChecks && com.duochat.app.data.AppUpdate.mayCheck(settings.network) &&
            com.duochat.app.data.AppUpdate.dueForCheck(com.duochat.app.UpdateSignal.lastCheck, now)
        ) {
            // Only a finished look counts: a failed one (no network yet) or one cut short is tried again at the next resume.
            com.duochat.app.AppUpdater.latest().onSuccess {
                com.duochat.app.UpdateSignal.lastCheck = now
                // "Later" puts off the build that was shown, not every build after it: a newer one is offered at once.
                if (it != null && com.duochat.app.data.AppUpdate.shouldOffer(it, settings.updateSnoozedUntil, settings.updateSnoozedFor, now)) update = it
            }
        }
    }
    update?.let { info ->
        com.duochat.app.ui.UpdateDialog(info) {
            update = null
            viewModel.updateSettings {
                copy(updateSnoozedUntil = System.currentTimeMillis() + 3 * 24 * 60 * 60 * 1000L, updateSnoozedFor = info.versionCode)
            }
        }
    }
    // Tell the engine which conversation is on screen (it marks messages read and stays quiet about them).
    val openChatId = route.takeIf { it.startsWith("chat:") }?.removePrefix("chat:")
    LaunchedEffect(openChatId, state?.phase) { if (state?.phase == EnginePhase.READY) viewModel.setOpenChat(openChatId) }

    // Ask for notification permission once the first friend exists (never before there is anything to be told about).
    val hasFriends = state?.contacts?.isNotEmpty() == true
    LaunchedEffect(hasFriends) {
        if (hasFriends && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    // Somebody typed our six words and answered: we confirm who it is before adding them.
    // Nothing from the chats (names, safety numbers) and no action is reachable over the app lock.
    if (!locked) state?.requests?.firstOrNull()?.let { request ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { viewModel.dismissRequest(request.uid) },
            title = { androidx.compose.material3.Text("${request.name} used your invite") },
            text = {
                androidx.compose.material3.Text(
                    "Only add them if you expected this. To be sure it is really them, read these numbers to ${request.name} on a call or in person; they should see the same ones.\n\n${request.safetyNumber}",
                )
            },
            confirmButton = {
                androidx.compose.material3.TextButton(onClick = { viewModel.acceptRequest(request.uid) { r -> r.onSuccess { route = "chat:$it" } } }) {
                    androidx.compose.material3.Text("Add ${request.name}")
                }
            },
            dismissButton = {
                androidx.compose.material3.TextButton(onClick = { viewModel.dismissRequest(request.uid) }) { androidx.compose.material3.Text("Not them") }
            },
        )
    }

    // Which screen is showing: the phase screens first, then the chat routes. Changing it animates (see Motion.kt).
    // The opening animation (LoadingScreen) always gets a moment to play, even when the engine is ready at once.
    var opened by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(Unit) { kotlinx.coroutines.delay(850); opened = true }
    val screen = when {
        locked -> "lock"
        !opened -> "loading"
        state == null || state.phase == EnginePhase.STOPPED || state.phase == EnginePhase.STARTING -> "loading"
        state.phase == EnginePhase.NEEDS_IDENTITY -> "setup"
        state.phase == EnginePhase.ERROR -> "error"
        else -> route
    }
    val ready = routeDepth(screen) >= 0
    if (ready && state != null) {
        // A chat or group that no longer exists sends the user home instead of showing an empty screen.
        if (!exists(state, route)) route = HOME
        // One back handler for the whole app: group details go back to their chat, everything else home.
        BackHandler(enabled = route != HOME) {
            route = if (route.startsWith("group:")) "chat:" + route.removePrefix("group:") else HOME
        }
    }
    val back = { route = HOME }

    androidx.compose.animation.AnimatedContent(
        targetState = screen,
        transitionSpec = { screenTransition() },
        contentKey = { it },
        label = "screen",
        modifier = Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background),
    ) { shown ->
        when {
        shown == "lock" -> com.duochat.app.ui.LockScreen(onUnlocked = viewModel::unlock)

        shown == "loading" -> LoadingScreen("Opening your encrypted local data…")

        shown == "setup" -> OnboardingScreen(
            onCreate = viewModel::createIdentity,
            state = state,
            onJoinDevice = viewModel::joinDeviceLink,
            onStartScanOffer = viewModel::startScanLinkOffer,
            onSubmitLinkCode = viewModel::submitLinkCode,
            onCancelDeviceLink = viewModel::cancelDeviceLink,
            onDeviceLinked = viewModel::onDeviceLinked,
        )

        shown == "error" -> FatalScreen(
            message = state?.error ?: "The chat engine could not start.",
            onRetry = viewModel::startEngine,
        )

        // A screen on its way out can outlive its data (a chat removed, the engine restarting): draw nothing for it.
        state == null || !exists(state, shown) -> androidx.compose.foundation.layout.Box(Modifier.fillMaxSize())

        else -> {
            val openChatId = shown.takeIf { it.startsWith("chat:") }?.removePrefix("chat:")
            when {
                shown == "add" -> {
                    // Leaving the screen ends the six-word invite (rotating it does not): while it is open, relays see it
                    // posted every few seconds.
                    val addContext = androidx.compose.ui.platform.LocalContext.current
                    androidx.compose.runtime.DisposableEffect(Unit) {
                        onDispose {
                            if ((addContext as? android.app.Activity)?.isChangingConfigurations != true) viewModel.closeInvite()
                        }
                    }
                    AddFriendScreen(
                        state = state,
                        ownCard = viewModel::ownCard,
                        onAdd = viewModel::addFriend,
                        onCreateInvite = viewModel::createInvite,
                        onRetryAnswer = viewModel::retryAnswer,
                        onClearAnswer = viewModel::clearAnswer,
                        onChecked = viewModel::markChecked,
                        onBack = back,
                        onOpenChat = { route = "chat:$it" },
                    )
                }

                shown == "newgroup" -> {
                    NewGroupScreen(
                        state = state,
                        onCreate = viewModel::createGroup,
                        onBack = back,
                        onCreated = { route = "chat:$it" },
                    )
                }

                shown.startsWith("group:") -> {
                    val id = shown.removePrefix("group:")
                    GroupDetailsScreen(
                        state = state,
                        groupId = id,
                        onBack = { route = "chat:$id" },
                        onAddMembers = { uids, done -> viewModel.addToGroup(id, uids, done) },
                        onRemove = { uid, done -> viewModel.removeFromGroup(id, uid, done) },
                        onRename = { name, done -> viewModel.renameGroup(id, name, done) },
                        onLeave = { done -> viewModel.leaveGroup(id, done) },
                        onVerify = { friendDialog = it },
                        onMessage = { route = "chat:$it" },
                        onLeft = { route = "chat:$id" },
                    )
                }

                openChatId != null -> {
                    val isGroup = state.groups.any { it.groupId == openChatId }
                    ChatScreen(
                        state = state,
                        conversationId = openChatId,
                        muteUntil = state.prefs.muteUntil[openChatId] ?: 0L,
                        silent = openChatId in state.prefs.silentChats,
                        onSetSilent = { on -> viewModel.setSilent(openChatId, on) },
                        loadReceipts = viewModel::messageReceiptsJson,
                        archived = openChatId in state.prefs.archived,
                        onBack = back,
                        onSend = { text, replyToId, done -> viewModel.send(openChatId, text, replyToId, done) },
                        onSchedule = { text, at, done -> viewModel.scheduleMessage(openChatId, text, at, done) },
                        onCancelScheduled = viewModel::cancelScheduled,
                        onSendFile = { name, mime, bytes, replyToId, done -> viewModel.sendFile(openChatId, name, mime, bytes, replyToId, done) },
                        loadFile = { id -> viewModel.getFile(id) },
                        onRetry = viewModel::retry,
                        onClearNotice = viewModel::clearNotice,
                        onOpenDetails = { if (isGroup) route = "group:$openChatId" else friendDialog = openChatId },
                        onVerify = { friendDialog = openChatId },
                        onMuteUntil = { until -> viewModel.setMuteUntil(openChatId, until) },
                        onSetArchived = { a -> viewModel.setArchived(openChatId, a); if (a) route = HOME },
                        quickTimes = viewModel::quickSendTimes,
                        onClearChat = { viewModel.clearChat(openChatId) },
                        onLeaveChannel = { viewModel.leaveChannel(openChatId); route = HOME },
                        onMuteSender = viewModel::muteSender,
                        onDeleteMessage = viewModel::deleteMessage,
                        onLoadEarlier = { viewModel.loadEarlier(openChatId) },
                        onReact = viewModel::react,
                        onEnableStoreForward = { viewModel.updateSettings { copy(network = network.copy(storeForward = true)) } },
                        onOpenConnectionSettings = { settingsOpen = true },
                        insight = viewModel::insight,
                        onConnectBluetooth = {
                            viewModel.connectBluetooth(openChatId) { result ->
                                result.exceptionOrNull()?.message?.let { settingsOpen = true; android.widget.Toast.makeText(context, it, android.widget.Toast.LENGTH_LONG).show() }
                            }
                        },
                    )
                }

                else -> HomeScreen(
                    state = state,
                    onOpenChat = { route = "chat:${it.id}" },
                    pinned = state.prefs.pins,
                    archived = state.prefs.archived,
                    onTogglePin = viewModel::togglePin,
                    silentChats = state.prefs.silentChats,
                    onAddFriend = { route = "add" },
                    onNewGroup = { route = "newgroup" },
                    onChannels = { channelsOpen = true },
                    onOpenSettings = { settingsOpen = true },
                    onClearNotice = viewModel::clearNotice,
                    onSearch = viewModel::search,
                    onPanic = if (settings.panicWipe) ({ panic = true; nukeOpen = true }) else null,
                    onOpenConversation = { route = "chat:$it" },
                    onRecreateGroup = { old -> viewModel.recreateLegacyGroup(old) { r -> r.onSuccess { route = "chat:$it" } } },
                    onDismissOldGroup = viewModel::dismissLegacyGroup,
                    whatsNew = whatsNew,
                    onDismissWhatsNew = { viewModel.updateSettings { copy(lastSeenVersion = AppConfig.APP_VERSION) } },
                )
            }
        }
        }
    }

    if (ready && state != null) {
            if (channelsOpen) {
                com.duochat.app.ui.ChannelsDialog(
                    channels = state.channels,
                    onJoin = viewModel::joinChannel,
                    onLeave = { id -> viewModel.leaveChannel(id); if (route == "chat:$id") route = HOME },
                    onOpen = { id -> route = "chat:$id" },
                    onDismiss = { channelsOpen = false },
                )
            }

            friendDialog?.let { uid ->
                val friend = state.contacts.firstOrNull { it.uid == uid }
                if (friend == null) {
                    friendDialog = null
                } else {
                    FriendDetailsDialog(
                        friend = friend,
                        onDismiss = { friendDialog = null },
                        onRename = { viewModel.renameFriend(uid, it) },
                        onChecked = { viewModel.markChecked(uid) },
                        onRemove = { done -> viewModel.removeFriend(uid) { r -> r.onSuccess { route = HOME }; done(r) } },
                        onRepair = { done -> viewModel.repairFriend(uid, done) },
                    )
                }
            }

            if (settingsOpen) {
                SettingsDialog(
                    state = state,
                    settings = settings,
                    onDismiss = { settingsOpen = false },
                    onRequestNotifications = requestNotifications,
                    onUpdateSettings = viewModel::updateSettings,
                    onSetQuietHours = viewModel::setQuietHours,
                    insight = viewModel::insight,
                    diagnosticsJson = viewModel::diagnosticsJson,
                    selfTestJson = viewModel::selfTestJson,
                    diagStatus = viewModel::diagStatusJson,
                    diagExport = viewModel::diagExport,
                    onDiagClear = viewModel::diagClear,
                    onSetDiagEnabled = viewModel::setDiagEnabled,
                    onSetSecureScreen = viewModel::setSecureScreen,
                    storageByChatJson = viewModel::storageByChatJson,
                    onClearLocalData = viewModel::clearLocalData,
                    onDeleteMessages = { ids -> ids.forEach { viewModel.deleteMessage(it, false) } },
                    onOpenManual = {
                        settingsOpen = false
                        manualOpen = true
                    },
                    onOpenNuke = { nukeOpen = true },
                    onBluetoothPermissionResult = viewModel::refreshBluetooth,
                    onExportBackup = viewModel::exportBackup,
                    onImportBackup = viewModel::importBackup,
                    onStartDeviceLink = viewModel::startDeviceLink,
                    onHostScannedLink = viewModel::hostScannedLink,
                    onConfirmDeviceLink = viewModel::confirmDeviceLink,
                    onCancelDeviceLink = viewModel::cancelDeviceLink,
                    onDeviceRemoval = viewModel::deviceRemoval,
                )
            }

            if (manualOpen) {
                ManualConnectDialog(
                    state = state,
                    onDismiss = {
                        if (!state.manual.connected) viewModel.cancelManual()
                        manualOpen = false
                    },
                    onConfirmPeer = {
                        viewModel.confirmManual()
                        manualOpen = false
                    },
                    onCreateOffer = viewModel::createManualOffer,
                    onApplyAnswer = viewModel::applyManualAnswer,
                    onAcceptOffer = viewModel::acceptManualOffer,
                )
            }
    }

    val crash by viewModel.crash.collectAsStateWithLifecycle()
    if (!locked) crash?.let { notice ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = viewModel::dismissCrash,
            title = { androidx.compose.material3.Text(if (notice.survived) "PrivCord hit an error" else "PrivCord closed unexpectedly") },
            text = {
                androidx.compose.foundation.layout.Column {
                    androidx.compose.material3.Text(
                        (if (notice.radioTurnedOff) "Bluetooth was turned off so the app could open. You can turn it back on in Settings. " else "") +
                            "Copy the details (or take a screenshot) and send them to whoever builds the app, so it can be fixed. " +
                            "They contain no messages or names." +
                            (if (notice.survived) " The app kept running." else " The app starts once you close this."),
                    )
                    // The details themselves, readable and scrollable, so a screenshot works even if copying does not.
                    androidx.compose.foundation.text.selection.SelectionContainer {
                        androidx.compose.material3.Text(
                            notice.details.take(4000),
                            style = MaterialTheme.typography.bodySmall.copy(fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace),
                            modifier = Modifier
                                .padding(top = 12.dp)
                                .heightIn(max = 220.dp)
                                .verticalScroll(androidx.compose.foundation.rememberScrollState()),
                        )
                    }
                }
            },
            confirmButton = {
                androidx.compose.material3.TextButton(onClick = {
                    val clipboard = context.getSystemService(android.content.ClipboardManager::class.java)
                    clipboard?.setPrimaryClip(android.content.ClipData.newPlainText("PrivCord crash", notice.details))
                    viewModel.dismissCrash()
                }) { androidx.compose.material3.Text("Copy details") }
            },
            dismissButton = { androidx.compose.material3.TextButton(onClick = viewModel::dismissCrash) { androidx.compose.material3.Text("OK") } },
        )
    }

    if (!locked && (nukeOpen || killSwitchReport != null)) {
        NukeDialog(
            autoRun = panic,
            onDismiss = { nukeOpen = false },
            onRun = { viewModel.runNuke() },
            report = killSwitchReport,
            onDone = {
                nukeOpen = false
                panic = false
                route = HOME
                viewModel.acknowledgeKillSwitch()
            },
        )
    }
}

/** Whether the conversation or group a route points at still exists (other routes always do). */
private fun exists(state: com.duochat.app.data.ChatState, route: String): Boolean = when {
    route.startsWith("chat:") -> route.removePrefix("chat:").let { id ->
        state.contacts.any { it.uid == id } || state.groups.any { it.groupId == id } || state.channels.any { it.id == id }
    }
    route.startsWith("group:") -> state.groups.any { it.groupId == route.removePrefix("group:") }
    else -> true
}
