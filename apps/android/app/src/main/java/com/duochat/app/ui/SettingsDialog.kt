package com.duochat.app.ui

import android.content.Intent
import android.graphics.Color as AndroidColor
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Notifications
import androidx.compose.material.icons.filled.Shield
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.coerceAtLeast
import androidx.compose.ui.unit.coerceIn
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.window.Dialog
import com.duochat.app.AppConfig
import com.duochat.app.ble.BlePermissions
import com.duochat.app.data.ACCENTS
import com.duochat.app.data.BluetoothStatus
import com.duochat.app.data.AppSettings
import com.duochat.app.data.ChatState
import com.duochat.app.data.NetworkConfig
import com.duochat.app.data.hasTurn
import com.duochat.app.data.SignalingConfig
import com.duochat.app.data.TurnServer

@Composable
fun SettingsDialog(
    state: ChatState,
    settings: AppSettings,
    onDismiss: () -> Unit,
    onRequestNotifications: () -> Unit,
    onUpdateSettings: (AppSettings.() -> AppSettings) -> Unit,
    onClearLocalData: ((Result<Unit>) -> Unit) -> Unit,
    onDeleteMessages: (List<String>) -> Unit = {},
    onSetQuietHours: (Boolean, Int, Int) -> Unit = { _, _, _ -> },
    insight: suspend (String, String) -> String = { _, _ -> "[]" },
    diagnosticsJson: suspend (String) -> String = { "{}" },
    selfTestJson: suspend () -> String = { "[]" },
    onSetSecureScreen: (Boolean) -> Unit = {},
    storageByChatJson: suspend () -> String = { "[]" },
    onOpenManual: () -> Unit,
    onOpenNuke: () -> Unit,
    onBluetoothPermissionResult: () -> Unit = {},
    onExportBackup: (String, (Result<String>) -> Unit) -> Unit = { _, _ -> },
    onImportBackup: (String, String, (Result<String>) -> Unit) -> Unit = { _, _, _ -> },
    onStartDeviceLink: ((Result<String>) -> Unit) -> Unit = { it(Result.failure(IllegalStateException("Not available."))) },
    onHostScannedLink: (String, (Result<Unit>) -> Unit) -> Unit = { _, done -> done(Result.failure(IllegalStateException("Not available."))) },
    onConfirmDeviceLink: (Boolean) -> Unit = {},
    onCancelDeviceLink: () -> Unit = {},
    onDeviceRemoval: (com.duochat.app.data.DeviceRemoval, (Result<Unit>) -> Unit) -> Unit = { _, done -> done(Result.failure(IllegalStateException("Not available."))) },
    diagStatus: suspend () -> String = { "{}" },
    diagExport: suspend (String) -> String = { "" },
    onDiagClear: suspend () -> Unit = {},
    onSetDiagEnabled: suspend (Boolean) -> Unit = {},
) {
    var linking by remember { mutableStateOf(false) }
    var displayName by remember { mutableStateOf(settings.displayName) }
    var nameStatus by remember { mutableStateOf<String?>(null) }
    val advanced = settings.advanced
    var stunText by remember { mutableStateOf(settings.network.stun.joinToString("\n")) }
    val firstTurn = settings.network.turn.firstOrNull()
    var turnUrls by remember { mutableStateOf(firstTurn?.urls?.joinToString("\n").orEmpty()) }
    var turnUser by remember { mutableStateOf(firstTurn?.username.orEmpty()) }
    var turnSecret by remember { mutableStateOf(firstTurn?.credential.orEmpty()) }
    var networkStatus by remember { mutableStateOf<String?>(null) }
    var nostrText by remember {
        mutableStateOf(
            settings.network.signaling.filterIsInstance<SignalingConfig.Nostr>().flatMap { it.relays }.joinToString("\n"),
        )
    }
    var relayText by remember {
        mutableStateOf(settings.network.signaling.filterIsInstance<SignalingConfig.Relay>().joinToString("\n") { it.url })
    }
    var clearConfirm by remember { mutableStateOf(false) }
    var clearStatus by remember { mutableStateOf<String?>(null) }
    var clearing by remember { mutableStateOf(false) }

    // A full page, not a floating box: settings are long, and a page reads calmer than a card over the chats.
    Dialog(
        onDismissRequest = onDismiss,
        properties = androidx.compose.ui.window.DialogProperties(
            usePlatformDefaultWidth = false,
            decorFitsSystemWindows = false,
            // This page is its own window: it needs the screen-privacy flag too.
            securePolicy = if (state.prefs.secureScreen) androidx.compose.ui.window.SecureFlagPolicy.SecureOn else androidx.compose.ui.window.SecureFlagPolicy.Inherit,
        ),
    ) {
        // Sizes come from the screen, not from one phone: side room grows with the width (16 to 32 dp), the text column
        // stops at a readable width on tablets, foldables and landscape, and the room at the end follows the height.
        val screen = LocalConfiguration.current
        val sidePad = (screen.screenWidthDp * 0.06f).dp.coerceIn(16.dp, 32.dp)
        val endRoom = (screen.screenHeightDp * 0.08f).dp.coerceAtLeast(48.dp)
        Surface(
            modifier = Modifier.fillMaxSize(),
            color = MaterialTheme.colorScheme.background,
        ) {
          Box(
            Modifier.fillMaxSize().windowInsetsPadding(androidx.compose.foundation.layout.WindowInsets.safeDrawing),
            contentAlignment = Alignment.TopCenter,
          ) {
            Column(
                modifier = Modifier
                    .widthIn(max = 640.dp)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = sidePad, vertical = 12.dp),
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        "Settings",
                        style = MaterialTheme.typography.headlineLarge,
                        modifier = Modifier.weight(1f),
                    )
                    IconButton(onClick = onDismiss) {
                        Icon(Icons.Filled.Close, contentDescription = "Close settings")
                    }
                }
                Spacer(Modifier.height(16.dp))
                // Who you are, at the top: the name friends see, on a card with your avatar.
                Surface(shape = RoundedCornerShape(24.dp), color = MaterialTheme.colorScheme.surfaceContainer, modifier = Modifier.fillMaxWidth()) {
                    Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                        InitialsAvatar(settings.displayName.ifBlank { "You" }, 56)
                        Spacer(Modifier.width(14.dp))
                        Column(Modifier.weight(1f)) {
                            Text(settings.displayName.ifBlank { "No name yet" }, style = MaterialTheme.typography.titleLarge)
                            Text(
                                "Most settings are for this device only. Quiet hours, mutes, pins and screen privacy follow you to your linked devices.",
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
                Spacer(Modifier.height(8.dp))

                Spacer(Modifier.height(8.dp))
                ToggleRow(
                    title = "Show advanced settings",
                    description = "Linked devices, connection options, diagnostics and power-user tools. Everything works without them.",
                    checked = settings.advanced,
                    onChange = { on -> onUpdateSettings { copy(advanced = on) } },
                )
                Spacer(Modifier.height(12.dp))

                // The things you may want to delete come first, so they are never a long scroll away.
                SectionTitle("Your data")
                Text(
                    "Messages are encrypted on this phone with a key in the Android Keystore and travel straight between " +
                        "phones; no server can read or keep them. Deleting here never touches your friends' phones.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(10.dp))
                if (clearConfirm) {
                    WrapRow(8.dp) {
                        Button(
                            onClick = {
                                clearing = true
                                onClearLocalData { result ->
                                    clearing = false
                                    clearConfirm = false
                                    clearStatus = result.fold(
                                        onSuccess = { "Local data cleared." },
                                        onFailure = { it.message ?: "Could not clear local data." },
                                    )
                                }
                            },
                            enabled = !clearing,
                        ) {
                            if (clearing) {
                                CircularProgressIndicator(
                                    modifier = Modifier.size(15.dp),
                                    strokeWidth = 2.dp,
                                )
                                Spacer(Modifier.width(8.dp))
                            }
                            Text("Yes, clear all messages")
                        }
                        TextButton(onClick = { clearConfirm = false }) {
                            Text("Cancel")
                        }
                    }
                } else {
                    OutlinedButton(onClick = { clearConfirm = true }) {
                        Text("Clear all messages")
                    }
                }
                clearStatus?.let {
                    Spacer(Modifier.height(6.dp))
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Spacer(Modifier.height(14.dp))
                Surface(
                    color = MaterialTheme.colorScheme.errorContainer.copy(alpha = 0.6f),
                    shape = RoundedCornerShape(16.dp),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Column(Modifier.padding(14.dp)) {
                        Text("Erase everything", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
                        Text(
                            "Permanently deletes your chats, friends, groups, keys and settings from this phone and starts over " +
                                "with a new identity. You will be asked to confirm twice.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(vertical = 6.dp),
                        )
                        Button(
                            onClick = {
                                onDismiss()
                                onOpenNuke()
                            },
                            colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error),
                        ) {
                            Text("Erase everything")
                        }
                    }
                }

                Spacer(Modifier.height(20.dp))

                SectionTitle("Profile")
                Text(
                    "There are no accounts. Your identity is a key that lives on this phone. " +
                        "Friends see this name on your invite and in groups.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(10.dp))
                DuoTextField(
                    value = displayName,
                    onValueChange = { displayName = it.take(40) },
                    label = { Text("Your name") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                Button(
                    onClick = {
                        onUpdateSettings { copy(displayName = displayName.trim()) }
                        nameStatus = "Saved. New invites and groups use this name."
                    },
                    enabled = displayName.trim() != settings.displayName,
                ) {
                    Text("Save name")
                }
                nameStatus?.let {
                    Spacer(Modifier.height(6.dp))
                    Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }

                Spacer(Modifier.height(20.dp))

SectionTitle("Appearance")
                Text(
                    "Color theme",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(8.dp))
                BoxWithConstraints(Modifier.fillMaxWidth()) {
                // About three to six tiles a row, filling the width exactly, whatever the screen.
                val columns = (maxWidth / 104.dp).toInt().coerceAtLeast(3)
                val tileWidth = (maxWidth - 8.dp * (columns - 1)) / columns
                @OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
                androidx.compose.foundation.layout.FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    ThemeTile(tileWidth, "Plum & paper", listOf(Color(0xFFF7F3EE), Color(0xFF7A3E73), Color(0xFFF2A65A), Color(0xFF131112)), settings.palette == com.duochat.app.data.CLASSIC_PALETTE) {
                        onUpdateSettings { copy(palette = com.duochat.app.data.CLASSIC_PALETTE) }
                    }
                    for (t in com.duochat.app.data.THEMES) {
                        ThemeTile(tileWidth, t.name, listOf(Color(t.bg), Color(t.surface), Color(com.duochat.app.data.accentFill(t.accent))), settings.palette == t.id) {
                            onUpdateSettings { copy(palette = t.id) }
                        }
                    }
                }
                }
                Spacer(Modifier.height(12.dp))
                if (settings.palette != com.duochat.app.data.CLASSIC_PALETTE) {
                    Text(
                        "The color theme sets the light or dark look and the accent color.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                } else {
                WrapRow(8.dp) {
                    ChoiceChip("System", settings.theme == "system") {
                        onUpdateSettings { copy(theme = "system") }
                    }
                    ChoiceChip("Dark", settings.theme == "dark") {
                        onUpdateSettings { copy(theme = "dark") }
                    }
                    ChoiceChip("Light", settings.theme == "light") {
                        onUpdateSettings { copy(theme = "light") }
                    }
                }
                Spacer(Modifier.height(12.dp))
                Text(
                    "Accent color",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(8.dp))
                WrapRow(4.dp) {
                    for ((label, value) in ACCENTS) {
                        Box(
                            modifier = Modifier
                                .minimumInteractiveComponentSize()
                                .size(36.dp)
                                .clip(CircleShape)
                                .background(parseColor(value))
                                .border(
                                    width = if (settings.accent == value) 3.dp else 1.dp,
                                    color = if (settings.accent == value) {
                                        MaterialTheme.colorScheme.onSurface
                                    } else {
                                        MaterialTheme.colorScheme.outline
                                    },
                                    shape = CircleShape,
                                )
                                .clickable { onUpdateSettings { copy(accent = value) } },
                            contentAlignment = Alignment.Center,
                        ) {
                            if (settings.accent == value) {
                                Icon(
                                    Icons.Filled.Check,
                                    contentDescription = label,
                                    tint = Color.White,
                                    modifier = Modifier.size(18.dp),
                                )
                            }
                        }
                    }
                }
                }

                Spacer(Modifier.height(20.dp))

                SectionTitle("Notifications")
                val healthCtx = LocalContext.current
                val healthPermission = androidx.core.app.NotificationManagerCompat.from(healthCtx).areNotificationsEnabled()
                val healthBattery = com.duochat.app.notifications.BackgroundHelp.batteryUnrestricted(healthCtx)
                val healthMinute = java.util.Calendar.getInstance().let { it.get(java.util.Calendar.HOUR_OF_DAY) * 60 + it.get(java.util.Calendar.MINUTE) }
                val healthQuietNow = com.duochat.app.data.inQuietNow(state.prefs.quiet, healthMinute)
                val healthMuted = com.duochat.app.data.activeMutes(state.prefs.muteUntil)
                val healthSignaling = when (state.signaling) {
                    com.duochat.app.signaling.SignalingStatus.IDLE -> "unconfigured"
                    com.duochat.app.signaling.SignalingStatus.CONNECTING -> "connecting"
                    com.duochat.app.signaling.SignalingStatus.ONLINE -> "online"
                    com.duochat.app.signaling.SignalingStatus.ERROR -> "offline"
                }
                val healthJson by androidx.compose.runtime.produceState(
                    "{}", healthSignaling, settings.notificationsEnabled, healthPermission, healthBattery, settings.keepRunning, healthQuietNow, healthMuted,
                ) {
                    value = insight(
                        "health",
                        com.duochat.app.data.healthInput(
                            healthSignaling, settings.notificationsEnabled, healthPermission, healthBattery, settings.keepRunning, healthQuietNow, healthMuted,
                        ),
                    )
                }
                com.duochat.app.data.parseHealth(healthJson)?.let { h ->
                    DeliveryHealthCard(h, onFix = { fix -> com.duochat.app.notifications.BackgroundHelp.openFix(healthCtx, fix) })
                    Spacer(Modifier.height(10.dp))
                }
                ToggleRow(
                    "Message notifications",
                    "Show a system notification for new messages.",
                    settings.notificationsEnabled,
                ) { onUpdateSettings { copy(notificationsEnabled = it) } }
                ToggleRow(
                    "Message previews",
                    "Include message text in notifications. Turn off for privacy.",
                    settings.notificationPreviews,
                    enabled = settings.notificationsEnabled,
                ) { onUpdateSettings { copy(notificationPreviews = it) } }
                val quiet = state.prefs.quiet
                ToggleRow(
                    "Quiet hours",
                    "No notifications between these times, on all your devices. Messages still arrive and wait in the chat.",
                    quiet.enabled,
                    enabled = settings.notificationsEnabled,
                ) { onSetQuietHours(it, quiet.from, quiet.to) }
                if (quiet.enabled && settings.notificationsEnabled) {
                    val ctx = LocalContext.current
                    fun pick(initial: Int, set: (Int) -> Unit) {
                        android.app.TimePickerDialog(ctx, { _, h, m -> set(h * 60 + m) }, initial / 60, initial % 60, true).show()
                    }
                    WrapRow(8.dp) {
                        OutlinedButton(onClick = { pick(quiet.from) { v -> onSetQuietHours(true, v, quiet.to) } }) {
                            Text("From ${com.duochat.app.data.minutesLabel(quiet.from)}")
                        }
                        OutlinedButton(onClick = { pick(quiet.to) { v -> onSetQuietHours(true, quiet.from, v) } }) {
                            Text("Until ${com.duochat.app.data.minutesLabel(quiet.to)}")
                        }
                    }
                }
                Spacer(Modifier.height(8.dp))
                if (healthPermission) {
                    Text(
                        "Android allows PrivCord to show notifications.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                } else {
                    OutlinedButton(onClick = onRequestNotifications) {
                        Icon(
                            Icons.Filled.Notifications,
                            contentDescription = null,
                            modifier = Modifier.size(16.dp),
                        )
                        Spacer(Modifier.width(8.dp))
                        Text("Allow notifications")
                    }
                }
                val bgContext = LocalContext.current
                ToggleRow(
                    "Keep running in the background",
                    "Shows a small permanent notification and keeps PrivCord connected so messages arrive with the app closed, also after a restart. Uses some battery.",
                    settings.keepRunning,
                ) { on ->
                    onUpdateSettings { copy(keepRunning = on) }
                    // Without this the battery saver stops it within minutes on most phones: ask right away.
                    if (on && !com.duochat.app.notifications.BackgroundHelp.batteryUnrestricted(bgContext)) {
                        com.duochat.app.notifications.BackgroundHelp.askBatteryUnrestricted(bgContext)
                    }
                }
                if (settings.keepRunning) {
                    val unrestricted = com.duochat.app.notifications.BackgroundHelp.batteryUnrestricted(bgContext)
                    OutlinedButton(onClick = { com.duochat.app.notifications.BackgroundHelp.askBatteryUnrestricted(bgContext) }) {
                        Text(if (unrestricted) "Battery: unrestricted ✓" else "Allow unrestricted battery use")
                    }
                    OutlinedButton(onClick = { com.duochat.app.notifications.BackgroundHelp.openMakerBackgroundSettings(bgContext) }) {
                        Text("Allow autostart (phone settings)")
                    }
                    Text(
                        "Vivo, Xiaomi, Oppo, Realme, OnePlus and Huawei phones also stop apps that are not allowed to " +
                            "autostart. If messages only arrive when you open PrivCord, turn on autostart / background " +
                            "running for PrivCord on the page this button opens.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Spacer(Modifier.height(6.dp))
                Text(
                    "There is no push service (that would need a server that knows when you get a message). Instead, " +
                        "PrivCord asks the relays for waiting messages about every ${com.duochat.app.notifications.BackgroundCheck.PERIOD_MS / 60_000} minutes even when it is closed, and " +
                        "continuously while it is open or \"Keep running\" is on. With store-and-forward on (Connection > " +
                        "Advanced options), messages wait on the relays for 7 days.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

                Spacer(Modifier.height(20.dp))

                SectionTitle("Connection")
                Text(
                    "Friends reconnect automatically whenever you are both online. Nothing here needs changing for that to work.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(8.dp))
                // The same words, in the same order, as the Windows app's Connection page.
                Text(
                    when (state.signaling) {
                        com.duochat.app.signaling.SignalingStatus.ONLINE -> "Online. Friends can find you."
                        com.duochat.app.signaling.SignalingStatus.CONNECTING -> "Connecting…"
                        com.duochat.app.signaling.SignalingStatus.IDLE -> "No connection service is on. Only Bluetooth and open connections work."
                        com.duochat.app.signaling.SignalingStatus.ERROR -> "Could not reach the connection service. Retrying."
                    },
                    style = MaterialTheme.typography.titleSmall,
                )
                Text(
                    "${state.linkCount} of ${state.contacts.size} friends connected" +
                        if (state.queuedCount > 0) " · ${state.queuedCount} message${if (state.queuedCount == 1) "" else "s"} waiting" else "",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(8.dp))
                if (advanced) {
                    Spacer(Modifier.height(8.dp))
                val hostsJson by androidx.compose.runtime.produceState("[]", settings.network, settings.updateChecks) {
                    value = insight("hosts", com.duochat.app.data.hostsInput(settings.network, settings.updateChecks))
                }
                NetworkHostsCard(com.duochat.app.data.parseHosts(hostsJson))
                Spacer(Modifier.height(8.dp))
                DetailRow("Your ID", state.uid.take(16) + "…", monospace = true)
                Text(
                    "PrivCord stays connected to at most 30 friends at once; beyond that the rest connect when others drop.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(10.dp))
                OutlinedButton(onClick = onOpenManual) {
                    Text("Connect with a code (no internet)")
                }
                Spacer(Modifier.height(4.dp))
                Text(
                    "There is no server to introduce your devices. One of you creates a code, " +
                        "the other pastes it and sends back a reply code. Once connected, " +
                        "messages go straight between your devices.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )


                    Spacer(Modifier.height(14.dp))
                    BluetoothBlock(
                        status = state.bluetooth,
                        onPermissionResult = onBluetoothPermissionResult,
                    )
                    Spacer(Modifier.height(14.dp))
                    val meshContext = LocalContext.current
                    val meshPermission = androidx.activity.compose.rememberLauncherForActivityResult(
                        androidx.activity.result.contract.ActivityResultContracts.RequestMultiplePermissions(),
                    ) { granted -> if (granted.values.all { it }) onUpdateSettings { copy(mesh = true) } }
                    ToggleRow(
                        title = "Bluetooth mesh (relay for nearby phones)",
                        description = "Pass messages along through nearby phones that also run PrivCord, so they can reach people several phones away with no internet, and join public channels over Bluetooth. Other phones can see that a PrivCord phone is nearby. Uses more battery. Experimental: simulated and compiled, never run on real radios." +
                            if (settings.mesh) " Linked phones now: ${state.meshLinks}. Heard ${state.meshHeard} new messages, passed on ${state.meshPassedOn} copies." else "",
                        checked = settings.mesh,
                        onChange = { on ->
                            if (!on) onUpdateSettings { copy(mesh = false) }
                            else if (BlePermissions.granted(meshContext)) onUpdateSettings { copy(mesh = true) }
                            else meshPermission.launch(BlePermissions.required())
                        },
                    )
                    Spacer(Modifier.height(14.dp))
                    SectionTitle("Communication Mode")
                Text("How PrivCord reaches your friends. Automatic tries Bluetooth when someone is close and the internet otherwise.", style = MaterialTheme.typography.bodySmall)
                Spacer(Modifier.height(4.dp))
                WrapRow(6.dp) {
                    ChoiceChip("Automatic", settings.network.transportMode == com.duochat.app.data.TransportMode.AUTOMATIC) {
                        onUpdateSettings { copy(network = network.copy(transportMode = com.duochat.app.data.TransportMode.AUTOMATIC)) }
                    }
                    ChoiceChip("Bluetooth only", settings.network.transportMode == com.duochat.app.data.TransportMode.BLUETOOTH_ONLY) {
                        onUpdateSettings { copy(network = network.copy(transportMode = com.duochat.app.data.TransportMode.BLUETOOTH_ONLY)) }
                    }
                    ChoiceChip("Internet only", settings.network.transportMode == com.duochat.app.data.TransportMode.INTERNET_ONLY) {
                        onUpdateSettings { copy(network = network.copy(transportMode = com.duochat.app.data.TransportMode.INTERNET_ONLY)) }
                    }
                }
                Spacer(Modifier.height(8.dp))
                SectionTitle("Network")
                Text(
                    "STUN only tells your device its public address; no messages pass through " +
                        "it. TURN relays your already end-to-end encrypted connection when no " +
                        "direct path exists (strict NAT or firewalls). Both are optional. " +
                        "Changes apply to the next connection.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(8.dp))
                DuoTextField(
                    value = nostrText,
                    onValueChange = { nostrText = it },
                    label = { Text("Signaling: Nostr relays (finds your friend; one per line)") },
                    minLines = 2,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                DuoTextField(
                    value = relayText,
                    onValueChange = { relayText = it },
                    label = { Text("Signaling: your own PrivCord relay (one per line)") },
                    minLines = 1,
                    modifier = Modifier.fillMaxWidth(),
                )
                Text(
                    "Signaling status: ${signalingWords(state.signaling)}. Leave both empty to use connection codes only. " +
                        "Use wss:// addresses (Android blocks plain ws://). Signaling only carries sealed, signed " +
                        "connection setup, never messages.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(8.dp))
                DuoTextField(
                    value = stunText,
                    onValueChange = { stunText = it },
                    label = { Text("STUN servers (one per line, stun:host:port)") },
                    minLines = 2,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                DuoTextField(
                    value = "",
                    onValueChange = { pasted ->
                        com.duochat.app.data.TurnShare.parse(pasted)?.let { t ->
                            turnUrls = t.urls.joinToString("\n"); turnUser = t.username; turnSecret = t.credential
                        }
                    },
                    label = { Text("Paste a relay setup line (PRIVCORD-TURN1:…)") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                DuoTextField(
                    value = turnUrls,
                    onValueChange = { turnUrls = it },
                    label = { Text("TURN URLs (turn:host:port or turns:…)") },
                    minLines = 1,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                DuoTextField(
                    value = turnUser,
                    onValueChange = { turnUser = it },
                    label = { Text("TURN username") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                DuoTextField(
                    value = turnSecret,
                    onValueChange = { turnSecret = it },
                    label = { Text("TURN password") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                if (com.duochat.app.p2p.hasBuiltinTurn) ToggleRow(
                    "PrivCord relay when a direct link fails",
                    "Some networks block direct connections. The relay then passes the encrypted traffic on; it cannot read it.",
                    settings.network.builtinTurn,
                ) { onUpdateSettings { copy(network = network.copy(builtinTurn = it)) } }
                // Two levels of hiding this phone's address: from friends (a TURN relay carries every link), or from
                // everyone (Tor; slower, no live links).
                val turnAvailable = settings.network.hasTurn
                ToggleRow(
                    "Hide my IP address from friends",
                    if (turnAvailable) {
                        "Every connection goes through a relay (TURN), so friends only ever see the relay's address. " +
                            "A little slower. The relay sees your address, not your messages."
                    } else {
                        "Needs a relay (TURN) server, and none is set up. Add one below, or use Private mode (Tor), which needs none."
                    },
                    settings.network.forceRelay && turnAvailable,
                    enabled = turnAvailable && !settings.network.torMode,
                ) { onUpdateSettings { copy(network = network.copy(forceRelay = it)) } }
                ToggleRow(
                    "Private mode (Tor): hide my IP from everyone",
                    "Messages go only through Tor and the relays, so neither friends nor relays learn this phone's " +
                        "address. Needs the free Orbot app running. Messages take a few seconds; live connections " +
                        "and sending files are off. Bluetooth still works.",
                    settings.network.torMode,
                ) { onUpdateSettings { copy(network = network.copy(torMode = it)) } }
                ToggleRow(
                    "Store-and-forward (messages while your friend is offline)",
                    "Sends each message, encrypted for your friend, to the relays above so it arrives when they come " +
                        "online. Off by default. Each message is sealed to a one-time key your friend's device erases after opening it, " +
                        "so a stolen phone cannot open it later; when those keys run out a standing key (replaced weekly) is used. " +
                        "The relays see that a sealed message for a mailbox arrived, not what it says.",
                    settings.network.storeForward,
                ) { onUpdateSettings { copy(network = network.copy(storeForward = it)) } }
                ToggleRow(
                    "Bluetooth (works with no internet)",
                    "Also look for your friend over Bluetooth LE when you are close by. Experimental: wired and unit-tested, never run on real radios.",
                    settings.network.bluetooth,
                ) { onUpdateSettings { copy(network = network.copy(bluetooth = it)) } }
                Spacer(Modifier.height(4.dp))
                WrapRow(8.dp) {
                    Button(
                        onClick = {
                            val urls = NetworkConfig.parseTurnUrls(turnUrls)
                            val hasTurnInput = turnUrls.isNotBlank() || turnUser.isNotBlank() || turnSecret.isNotBlank()
                            val nostrLines = nostrText.split('\n', ',', ' ').map { it.trim() }.filter { it.isNotEmpty() }
                            val relayLines = relayText.split('\n', ',', ' ').map { it.trim() }.filter { it.isNotEmpty() }
                            val badWs = (nostrLines + relayLines).firstOrNull { !NetworkConfig.isWsUrl(it) }
                            if (badWs != null) {
                                networkStatus = "Not a ws:// or wss:// address: $badWs"
                            } else if (hasTurnInput && (urls.isEmpty() || turnUser.isBlank() || turnSecret.isBlank())) {
                                networkStatus = "TURN needs at least one turn:/turns: URL, a username and a credential."
                            } else {
                                val turn = if (urls.isEmpty()) {
                                    emptyList()
                                } else {
                                    listOf(TurnServer(urls, turnUser.trim(), turnSecret))
                                }
                                onUpdateSettings {
                                    copy(
                                        network = network.copy(
                                            stun = NetworkConfig.parseStunList(stunText),
                                            turn = turn,
                                            signaling = buildList {
                                                if (nostrLines.isNotEmpty()) add(SignalingConfig.Nostr(nostrLines))
                                                relayLines.forEach { add(SignalingConfig.Relay(it)) }
                                            },
                                        ),
                                    )
                                }
                                networkStatus = "Saved. Applies to the next connection."
                            }
                        },
                    ) {
                        Text("Save network settings")
                    }
                    TextButton(
                        onClick = {
                            val defaults = AppConfig.defaultNetworkConfig()
                            onUpdateSettings { copy(network = defaults) }
                            stunText = defaults.stun.joinToString("\n")
                            nostrText = defaults.signaling.filterIsInstance<SignalingConfig.Nostr>().flatMap { it.relays }.joinToString("\n")
                            relayText = ""
                            turnUrls = defaults.turn.firstOrNull()?.urls?.joinToString("\n").orEmpty()
                            turnUser = defaults.turn.firstOrNull()?.username.orEmpty()
                            turnSecret = defaults.turn.firstOrNull()?.credential.orEmpty()
                            networkStatus = "Defaults restored."
                        },
                    ) {
                        Text("Reset to defaults")
                    }
                }
                networkStatus?.let {
                    Spacer(Modifier.height(6.dp))
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Text(
                    "The default STUN server is a public Cloudflare server (a third " +
                        "party, which sees this phone's address when a connection starts). Clear the list to " +
                        "disable STUN; then only same-network connections work.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                }

                Spacer(Modifier.height(20.dp))

SectionTitle("Security")
                val staleCount = state.devices.count { it.stale }
                val checkupJson by androidx.compose.runtime.produceState(
                    "[]", settings.appLock, settings.lastBackupAt, com.duochat.app.data.hidesIp(settings.network), settings.notificationPreviews,
                    settings.groupOnly, settings.panicWipe, staleCount,
                ) {
                    value = insight(
                        "checkup",
                        com.duochat.app.data.checkupInput(
                            settings.appLock, settings.lastBackupAt, com.duochat.app.data.hidesIp(settings.network), !settings.notificationPreviews,
                            settings.groupOnly, settings.panicWipe, staleCount,
                        ),
                    )
                }
                PrivacyCheckupCard(
                    items = com.duochat.app.data.parseCheckup(checkupJson),
                    canTurnOn = setOf("lock", "previews", "grouponly", "panic"),
                    onTurnOn = { id ->
                        onUpdateSettings {
                            when (id) {
                                "lock" -> copy(appLock = true)
                                "previews" -> copy(notificationPreviews = false)
                                "grouponly" -> copy(groupOnly = true)
                                "panic" -> copy(panicWipe = true)
                                else -> this
                            }
                        }
                    },
                )
                Spacer(Modifier.height(12.dp))
                val appContext = LocalContext.current
                val lockAvailable = remember { com.duochat.app.security.AppLock.available(appContext) }
                ToggleRow(
                    title = "Lock with fingerprint, face or screen lock",
                    description = if (lockAvailable) {
                        "Ask for it every time PrivCord opens, and again after it has been out of sight for ${com.duochat.app.security.AppLock.GRACE_MS / 1000} seconds. It keeps other people who pick up your phone out; it does not replace the encryption of your chats."
                    } else {
                        "Not available: set a screen lock (PIN, pattern or password) or a fingerprint in your phone's settings first."
                    },
                    checked = settings.appLock && lockAvailable,
                    enabled = lockAvailable,
                    onChange = { value ->
                        if (value) {
                            onUpdateSettings { copy(appLock = true) }
                        } else {
                            // Turning it off needs the same proof as opening the app.
                            // Inside a Dialog the context is a ContextThemeWrapper, not the activity: unwrap it. With no
                            // activity there is no prompt, so the lock stays on (never turned off without proof).
                            val activity = generateSequence(appContext) { (it as? android.content.ContextWrapper)?.baseContext }
                                .firstNotNullOfOrNull { it as? androidx.fragment.app.FragmentActivity }
                            if (activity != null) {
                                com.duochat.app.security.AppLock.prompt(activity, "Turn off app lock") { ok, _ ->
                                    if (ok) onUpdateSettings { copy(appLock = false) }
                                }
                            }
                        }
                    },
                )
                Spacer(Modifier.height(10.dp))
                ToggleRow(
                    "Hide the app from screenshots and the app switcher",
                    "Screenshots, screen recordings and the recent-apps preview show a blank screen. Applies to your chats, on every device you link.",
                    state.prefs.secureScreen,
                ) { on -> onSetSecureScreen(on) }
                Spacer(Modifier.height(14.dp))
                if (advanced) {
                ToggleRow(
                    title = "Stay group-only with people I meet in groups",
                    description = "You still read and reply in the groups you share, but nobody you only know through a group can message you directly, and you cannot message them, unless you swap invites yourselves.",
                    checked = settings.groupOnly,
                    onChange = { value -> onUpdateSettings { copy(groupOnly = value) } },
                )
                Spacer(Modifier.height(10.dp))
ToggleRow(
                    "Panic wipe",
                    "Tap the \"Chats\" title on the home screen three times quickly and everything on this phone is erased at once, with no confirmation. Your 12 recovery words are the only way back, so keep them somewhere safe first.",
                    settings.panicWipe,
                ) { on -> onUpdateSettings { copy(panicWipe = on) } }
                                }
                SectionTitle("Backup")
                BackupSection({ pass, done -> onExportBackup(pass) { r -> if (r.isSuccess) onUpdateSettings { copy(lastBackupAt = System.currentTimeMillis()) }; done(r) } }, onImportBackup)
                Spacer(Modifier.height(20.dp))

SectionTitle("Updates")
                ToggleRow(
                    "Check for updates",
                    "Looks for a new version when the app starts, when you come back to it, and about every ${com.duochat.app.data.AppUpdate.RECHECK_MS / 3_600_000} hours while it is open, and offers to install it. This contacts GitHub, which can see your IP address, so it stays off while Hide my IP or Tor mode is on.",
                    settings.updateChecks,
                ) { onUpdateSettings { copy(updateChecks = it) } }
                UpdateCheckRow(settings.network)
                Spacer(Modifier.height(20.dp))

                if (advanced) {
                SectionTitle("Devices")
                val devicesLine by androidx.compose.runtime.produceState<String?>(null, state.devices) {
                    value = com.duochat.app.data.parseDevicesLine(insight("devices_summary", com.duochat.app.data.devicesInput(state.devices)))
                }
                devicesLine?.let {
                    DevicesStrip(it)
                    Spacer(Modifier.height(10.dp))
                }
                DevicesSection(state, onLinkNew = { onCancelDeviceLink(); linking = true }, onRemoval = onDeviceRemoval)
                if (linking) {
                    LinkHostDialog(
                        state = state,
                        onStart = onStartDeviceLink,
                        onScanned = onHostScannedLink,
                        onConfirm = onConfirmDeviceLink,
                        onCancel = onCancelDeviceLink,
                        onClose = { linking = false },
                    )
                }

                Spacer(Modifier.height(20.dp))

val storageJson by androidx.compose.runtime.produceState("[]", state.messages) { value = storageByChatJson() }
                StorageByChatCard(
                    rows = com.duochat.app.data.parseStorage(storageJson) { id ->
                        state.contacts.firstOrNull { it.uid == id }?.name ?: state.groups.firstOrNull { it.groupId == id }?.name
                            ?: state.channels.firstOrNull { it.id == id }?.let { "# ${it.name}" } ?: "Chat"
                    },
                    onDeleteFiles = onDeleteMessages,
                )
                Spacer(Modifier.height(20.dp))

SectionTitle("Diagnostic log")
                DiagLogSection(status = diagStatus, export = diagExport, clear = onDiagClear, setEnabled = onSetDiagEnabled)
                Spacer(Modifier.height(20.dp))

                }
SectionTitle("About PrivCord")
                Text(
                    "Private messaging for friends and groups, with no accounts and no server.",
                    style = MaterialTheme.typography.bodyMedium,
                )
                Spacer(Modifier.height(6.dp))
                DetailRow("Version", com.duochat.app.BuildConfig.VERSION_NAME)
                DetailRow("Created by", AppConfig.CREATED_BY)
                DetailRow("License", AppConfig.LICENSE_SHORT)
                if (advanced) {
                Text("On this phone", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                DetailRow("Messages", state.usage.messages.toString())
                DetailRow("Friends", state.usage.contacts.toString())
                DetailRow("Files and photos", "${state.usage.files} · " + android.text.format.Formatter.formatShortFileSize(androidx.compose.ui.platform.LocalContext.current, state.usage.fileBytes))
                if (state.usage.syncEntries > 0) DetailRow("Kept for syncing your devices", "${state.usage.syncEntries} entries")
                Spacer(Modifier.height(10.dp))
                DetailRow("Platform", "Android · Kotlin/Compose")
                DetailRow("Rust core", com.duochat.app.RustCore.status)
                val context = androidx.compose.ui.platform.LocalContext.current
                val probe by androidx.compose.runtime.produceState("running…") {
                    value = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Default) {
                        com.duochat.app.rustcore.RustEngineProbe.run(context)
                    }
                }
                DetailRow("Rust engine", probe)
                    Spacer(Modifier.height(12.dp))
                    AboutTools(settings.network, insight, diagnosticsJson, selfTestJson)
                }
                Spacer(Modifier.height(12.dp))
                Text(
                    "Connections use a challenge-response identity proof, and you can compare a safety " +
                        "number with a friend. No accounts and no analytics.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                // This page is its own full-screen window, and on some phones the system does not report the navigation
                // bar to it, so the last lines would sit under the bar. Room at the end that does not depend on that.
                Spacer(Modifier.height(endRoom))
            }
          }
        }
    }
}

private fun signalingWords(s: com.duochat.app.signaling.SignalingStatus) = when (s) {
    com.duochat.app.signaling.SignalingStatus.IDLE -> "not set up"
    com.duochat.app.signaling.SignalingStatus.CONNECTING -> "connecting"
    com.duochat.app.signaling.SignalingStatus.ONLINE -> "online"
    com.duochat.app.signaling.SignalingStatus.ERROR -> "could not connect"
}

@Composable
private fun SectionTitle(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.titleLarge,
        modifier = Modifier.padding(top = 12.dp, bottom = 10.dp),
    )
}

@Composable
private fun DetailRow(label: String, value: String, monospace: Boolean = false) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(vertical = 3.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Text(
            label,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.weight(0.42f).padding(end = 12.dp),
        )
        Text(
            value,
            style = MaterialTheme.typography.bodySmall,
            fontFamily = if (monospace) FontFamily.Monospace else null,
            modifier = Modifier.weight(0.58f),
        )
    }
}

@Composable
internal fun ToggleRow(
    title: String,
    description: String,
    checked: Boolean,
    enabled: Boolean = true,
    onChange: (Boolean) -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).padding(end = 16.dp)) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            Text(
                description,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Switch(
            checked = checked,
            onCheckedChange = onChange,
            enabled = enabled,
        )
    }
}

@Composable
private fun ChoiceChip(label: String, selected: Boolean, onClick: () -> Unit) {
    Surface(
        shape = RoundedCornerShape(20.dp),
        color = if (selected) {
            MaterialTheme.colorScheme.onBackground
        } else {
            MaterialTheme.colorScheme.surfaceContainerHigh
        },
        modifier = Modifier.clickable(onClick = onClick),
    ) {
        Text(
            label,
            style = MaterialTheme.typography.bodyMedium,
            color = if (selected) {
                MaterialTheme.colorScheme.background
            } else {
                MaterialTheme.colorScheme.onSurfaceVariant
            },
            modifier = Modifier.padding(horizontal = 18.dp, vertical = 12.dp),
        )
    }
}

private fun parseColor(value: String): Color = try {
    Color(AndroidColor.parseColor(value))
} catch (_: Exception) {
    Color(0xFF7A3E73)
}

/**
 * Bluetooth LE status and controls. Permissions are requested only from the button here, after an explanation,
 * and a denial is handled with a pointer to the system settings. UNTESTED on a real radio.
 */
@Composable
private fun BluetoothBlock(
    status: BluetoothStatus,
    onPermissionResult: () -> Unit,
) {
    val context = LocalContext.current
    var rationale by remember { mutableStateOf(false) }
    var denied by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { result ->
        denied = !result.values.all { it }
        onPermissionResult()
    }

    DetailRow(
        "Bluetooth",
        when (status) {
            BluetoothStatus.DISABLED -> "Off in network settings"
            BluetoothStatus.UNSUPPORTED -> "Not available on this phone"
            BluetoothStatus.NEEDS_PERMISSION -> "Permission needed"
            BluetoothStatus.ADAPTER_OFF -> "Turned off"
            BluetoothStatus.NOT_PAIRED -> "Add a friend first"
            BluetoothStatus.READY -> "Ready"
            BluetoothStatus.SEARCHING -> "Searching nearby…"
            BluetoothStatus.CONNECTED -> "Connected"
        },
    )
    Text(
        when (status) {
            BluetoothStatus.NEEDS_PERMISSION ->
                "PrivCord can find your friends over Bluetooth when you are close by, with no internet. " +
                    "It needs the nearby-devices permission for that."
            BluetoothStatus.ADAPTER_OFF -> "Turn Bluetooth on in the quick settings panel, then come back."
            BluetoothStatus.CONNECTED -> "Traffic is encrypted on top of Bluetooth and never touches a server."
            else -> "Works with the internet off, but only within radio range. To use it, open a friend's chat menu and choose Connect over Bluetooth. Experimental: not tested on real radios."
        },
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    Spacer(Modifier.height(8.dp))
    WrapRow(8.dp) {
        if (status == BluetoothStatus.NEEDS_PERMISSION) {
            Button(onClick = { rationale = true }) { Text("Allow Bluetooth") }
            if (denied) {
                OutlinedButton(
                    onClick = {
                        context.startActivity(
                            Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
                                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                        )
                    },
                ) { Text("Open app settings") }
            }
        }
    }
    if (denied && status == BluetoothStatus.NEEDS_PERMISSION) {
        Spacer(Modifier.height(4.dp))
        Text(
            "Permission was denied. You can allow it under Android Settings > Apps > PrivCord > Permissions.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.error,
        )
    }
    message?.let {
        Spacer(Modifier.height(4.dp))
        Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
    }
    // What the radio did lately (advertising, scans, connections, with Android's codes), so a test that fails says why.
    val radioLog by androidx.compose.runtime.produceState(com.duochat.app.ble.BleLog.recent()) {
        while (true) {
            value = com.duochat.app.ble.BleLog.recent()
            kotlinx.coroutines.delay(2_000)
        }
    }
    if (radioLog.isNotEmpty()) {
        Spacer(Modifier.height(8.dp))
        Text("What Bluetooth did lately", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        androidx.compose.foundation.text.selection.SelectionContainer {
            Text(
                radioLog.joinToString("\n"),
                style = MaterialTheme.typography.bodySmall,
                fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
    if (rationale) {
        AlertDialog(
            onDismissRequest = { rationale = false },
            title = { Text("Allow nearby devices?") },
            text = {
                Text(
                    "Bluetooth lets PrivCord find and talk to your friends with no internet. PrivCord only " +
                        "connects to people you added. It never uses Bluetooth to learn your location.",
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        rationale = false
                        launcher.launch(BlePermissions.required())
                    },
                ) { Text("Continue") }
            },
            dismissButton = { TextButton(onClick = { rationale = false }) { Text("Not now") } },
        )
    }
}

/** One colour theme in the picker: a small preview of its colours and its name. */
@Composable
private fun ThemeTile(width: androidx.compose.ui.unit.Dp, name: String, colors: List<Color>, selected: Boolean, onClick: () -> Unit) {
    Column(
        modifier = Modifier
            .width(width)
            .clip(RoundedCornerShape(16.dp))
            .border(
                width = if (selected) 2.dp else 1.dp,
                color = if (selected) MaterialTheme.colorScheme.onBackground else MaterialTheme.colorScheme.outlineVariant,
                shape = RoundedCornerShape(16.dp),
            )
            .clickable(onClick = onClick)
            .padding(8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().height(40.dp).clip(RoundedCornerShape(10.dp)),
        ) {
            colors.forEach { c -> Box(Modifier.weight(1f).fillMaxHeight().background(c)) }
        }
        Spacer(Modifier.height(4.dp))
        Text(
            name,
            style = MaterialTheme.typography.labelSmall,
            maxLines = 1,
            overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
            color = if (selected) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
