package com.duochat.app.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.duochat.app.data.ChatState
import com.duochat.app.data.DeviceInfo
import com.duochat.app.data.DeviceLinkState
import com.duochat.app.data.DeviceRemoval
import java.text.DateFormat
import java.util.Date

/*
 * Linking another device of the account (docs/design/syncengine.md §2). The device holding the account key shows a
 * code (a QR code); the new device scans it; both show the same 6 digits; the person confirms on the first device.
 * The account key never leaves it.
 */

/** The account's devices, in Settings, with "Link a new device" on the device that holds the account key. */
@Composable
fun DevicesSection(
    state: ChatState,
    onLinkNew: () -> Unit,
    onRemoval: (DeviceRemoval, (Result<Unit>) -> Unit) -> Unit = { _, done -> done(Result.success(Unit)) },
) {
    var removing by remember { mutableStateOf<DeviceInfo?>(null) }
    Text(
        if (state.holdsAccountKey) {
            "Your chats stay in step on every device you link, with no server in between. Your account key stays on this phone."
        } else {
            "This device was linked to your account. Linking more devices is done from the device that holds your account key."
        },
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    Spacer(Modifier.height(10.dp))
    if (state.devices.isEmpty()) {
        Text("Only this device so far.", style = MaterialTheme.typography.bodyMedium)
    }
    val date = remember { DateFormat.getDateInstance(DateFormat.MEDIUM) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (d in state.devices) {
            Surface(shape = RoundedCornerShape(14.dp), color = MaterialTheme.colorScheme.surfaceContainer, modifier = Modifier.fillMaxWidth()) {
                Row(Modifier.padding(horizontal = 14.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(d.name + if (d.thisDevice) " (this device)" else "", style = MaterialTheme.typography.bodyLarge)
                        Text(
                            "Since " + date.format(Date(d.addedAt)),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        val removalAt = d.removalAt
                        if (removalAt != null) {
                            Text(
                                "Will be removed " + DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(removalAt)),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.error,
                            )
                        } else if (d.stale && !d.thisDevice) {
                            Text(
                                "Not seen for 90 days. Remove it, or open the app on it to reconnect.",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                    if (!d.thisDevice) {
                        if (d.removalAt != null) {
                            TextButton(onClick = { onRemoval(DeviceRemoval.Cancel(d.id)) {} }) { Text("Keep it") }
                        } else {
                            TextButton(onClick = { removing = d }) { Text("Remove") }
                        }
                    }
                }
            }
        }
    }
    if (state.syncLost > 0) {
        Spacer(Modifier.height(8.dp))
        Text(
            if (state.syncLost == 1L) "1 item from another device could not be recovered (that device was gone before it was sent on)."
            else "${state.syncLost} items from another device could not be recovered (that device was gone before they were sent on).",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
    if (state.holdsAccountKey) {
        Spacer(Modifier.height(10.dp))
        Button(onClick = onLinkNew) { Text("Link a new device") }
    }
    removing?.let { d -> RemoveDeviceDialog(d, onRemoval, onClose = { removing = null }) }
}

/** Removing a device: in 24 hours (every device sees it and can take it back), or at once with the recovery phrase. */
@Composable
private fun RemoveDeviceDialog(
    device: DeviceInfo,
    onRemoval: (DeviceRemoval, (Result<Unit>) -> Unit) -> Unit,
    onClose: () -> Unit,
) {
    var phrase by remember { mutableStateOf("") }
    var passphrase by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val run = { action: DeviceRemoval ->
        busy = true
        error = null
        onRemoval(action) { r ->
            busy = false
            r.onSuccess { onClose() }.onFailure { error = it.message }
        }
    }
    AlertDialog(
        onDismissRequest = { if (!busy) onClose() },
        title = { Text("Remove ${device.name}?") },
        text = {
            Column {
                Text(
                    "It stops getting your messages and is told to erase everything on it the next time it connects " +
                        "(that cannot be guaranteed if it never does). Your chats on your other devices stay as they are.",
                )
                Spacer(Modifier.height(8.dp))
                Text(
                    "Every device sees the request and can take it back for a day, so a stolen device cannot quietly " +
                        "remove the others. To remove it right away, enter your recovery phrase (it is not kept).",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(8.dp))
                DuoTextField(
                    value = phrase,
                    onValueChange = { phrase = it },
                    label = { Text("Recovery phrase (optional)") },
                    modifier = Modifier.fillMaxWidth(),
                )
                if (phrase.isNotBlank()) {
                    Spacer(Modifier.height(8.dp))
                    DuoTextField(
                        value = passphrase,
                        onValueChange = { passphrase = it },
                        label = { Text("Passphrase (only if your account has one)") },
                        singleLine = true,
                        visualTransformation = androidx.compose.ui.text.input.PasswordVisualTransformation(),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                error?.let {
                    Spacer(Modifier.height(8.dp))
                    Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                }
            }
        },
        confirmButton = {
            val withPhrase = phrase.trim().split(Regex("\\s+")).size >= 12
            Button(
                enabled = !busy,
                onClick = { run(if (withPhrase) DeviceRemoval.Now(device.id, phrase, passphrase) else DeviceRemoval.Request(device.id)) },
            ) { Text(if (withPhrase) "Remove now" else "Remove in 24 hours") }
        },
        dismissButton = { TextButton(enabled = !busy, onClick = onClose) { Text("Cancel") } },
    )
}

/**
 * On the device holding the account key. Scan the QR the new device shows (like WhatsApp Web), then the six digits
 * appear here: type them on the new device and tap Link. "Show a code instead" keeps the old way, for a new device that
 * cannot show a QR.
 */
@Composable
fun LinkHostDialog(
    state: ChatState,
    onStart: ((Result<String>) -> Unit) -> Unit,
    onScanned: (String, (Result<Unit>) -> Unit) -> Unit,
    onConfirm: (Boolean) -> Unit,
    onCancel: () -> Unit,
    onClose: () -> Unit,
) {
    var code by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var scanned by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    val scan = rememberQrScanner("Scan the code on the new device") { text ->
        busy = true
        error = null
        onScanned(text.trim()) { r ->
            busy = false
            r.onSuccess { scanned = true }.onFailure { error = it.message ?: "That is not a code from PrivCord." }
        }
    }
    val link = state.deviceLink
    val finished = link != null && link.stage != "code"
    AlertDialog(
        onDismissRequest = { if (!finished) onCancel(); onClose() },
        title = { Text("Link a new device") },
        text = {
            Column {
                when {
                    finished -> Text(endText(link, host = true))
                    link?.stage == "code" && link.code != null && scanned -> {
                        Text("Link ${link.name ?: "this device"} to your account? Type these digits on it.")
                        Digits(link.code)
                        Text(
                            "Only tap Link if you started this on your own computer. Never read these digits out to anyone.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    link?.stage == "code" && link.code != null -> {
                        Text("${link.name ?: "A device"} wants to join. Does it show these digits?")
                        Digits(link.code)
                    }
                    code != null -> {
                        val bitmap = remember(code) { qrBitmap(code ?: "") }
                        if (bitmap != null) {
                            Image(
                                bitmap.asImageBitmap(),
                                contentDescription = "Code for the new device",
                                modifier = Modifier.size(220.dp).align(Alignment.CenterHorizontally),
                            )
                        }
                        Spacer(Modifier.height(10.dp))
                        Text("On the new device, choose \"Link to a device you already use\" and scan this code, or type it:")
                        Spacer(Modifier.height(6.dp))
                        SelectionContainer {
                            Text(code ?: "", fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
                        }
                        Spacer(Modifier.height(6.dp))
                        Text(
                            "It works once, for ten minutes. Your account key stays on this phone.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    scanned -> Text("Waiting for the new device…")
                    else -> {
                        Text("On the new device, choose \"Link to a device I already use\". It shows a code. Scan it with this phone.")
                        Spacer(Modifier.height(12.dp))
                        Button(enabled = !busy, onClick = scan, modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp)) {
                            Text("Scan the new device")
                        }
                        Spacer(Modifier.height(4.dp))
                        TextButton(onClick = { onStart { r -> r.onSuccess { code = it }.onFailure { error = it.message } } }) {
                            Text("Show a code instead")
                        }
                    }
                }
                error?.let {
                    Spacer(Modifier.height(8.dp))
                    Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                }
            }
        },
        confirmButton = {
            if (link?.stage == "code" && !finished) {
                Button(onClick = { onConfirm(true) }) { Text(if (scanned) "Link it" else "They match, link it") }
            } else {
                TextButton(onClick = { if (!finished) onCancel(); onClose() }) { Text(if (finished) "Done" else "Cancel") }
            }
        },
        dismissButton = {
            if (link?.stage == "code" && !finished) {
                OutlinedButton(onClick = { onConfirm(false); onClose() }) { Text(if (scanned) "Cancel" else "They don't match") }
            }
        },
    )
}

/** On the new device (first run): scan or type the code, name the device, compare the digits, wait. */
@Composable
fun LinkJoinContent(
    state: ChatState?,
    onJoin: (code: String, name: String, onResult: (Result<Unit>) -> Unit) -> Unit,
    onStartScanOffer: (name: String, onResult: (Result<String>) -> Unit) -> Unit,
    onSubmitLinkCode: (digits: String, onResult: (Result<Boolean>) -> Unit) -> Unit,
    onCancel: () -> Unit,
    onLinked: () -> Unit,
    onBack: () -> Unit,
) {
    var offer by remember { mutableStateOf<String?>(null) }
    var digits by remember { mutableStateOf("") }
    var manual by remember { mutableStateOf(false) }
    var code by remember { mutableStateOf("") }
    var name by remember { mutableStateOf(android.os.Build.MODEL ?: "Phone") }
    var joined by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scan = rememberQrScanner("Scan the code on your other device") { code = it.trim() }
    val link = state?.deviceLink
    LaunchedEffect(link?.stage) { if ((joined || offer != null) && link?.stage == "done") onLinked() }

    Text("Link to a device you already use", style = MaterialTheme.typography.headlineSmall)
    Spacer(Modifier.height(12.dp))
    when {
        offer != null -> {
            val stage = link?.stage
            when {
                stage == "done" || stage == "failed" || stage == "cancelled" || stage == "expired" -> {
                    Text(endText(link, host = false))
                    Spacer(Modifier.height(16.dp))
                    if (stage != "done") OutlinedButton(onClick = { offer = null; digits = ""; onCancel() }) { Text("Try again") }
                }
                stage == "matched" -> Text("Digits match. Tap Link on your other device to finish…")
                stage == "enter-code" || stage == "wrong-code" -> {
                    Text("Your other device now shows six digits. Type them here.")
                    Spacer(Modifier.height(12.dp))
                    DuoTextField(
                        value = digits,
                        onValueChange = { v ->
                            val d = v.filter { it.isDigit() }.take(6)
                            digits = d
                            if (d.length == 6) onSubmitLinkCode(d) { r -> digits = ""; r.onFailure { error = it.message } }
                        },
                        label = { Text("Six digits") },
                        singleLine = true,
                        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(keyboardType = androidx.compose.ui.text.input.KeyboardType.NumberPassword),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    if (stage == "wrong-code") {
                        Spacer(Modifier.height(8.dp))
                        Text("Those digits are not right. Look at your other device and try again.", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                    }
                    error?.let {
                        Spacer(Modifier.height(8.dp))
                        Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                    }
                    Spacer(Modifier.height(12.dp))
                    TextButton(onClick = { offer = null; digits = ""; onCancel() }) { Text("Cancel") }
                }
                else -> {
                    val bitmap = remember(offer) { qrBitmap(offer ?: "") }
                    if (bitmap != null) {
                        androidx.compose.foundation.layout.Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                            Image(
                                bitmap.asImageBitmap(),
                                contentDescription = "Code for your other device to scan",
                                modifier = Modifier.size(220.dp),
                            )
                        }
                    }
                    Spacer(Modifier.height(10.dp))
                    Text("On your other device, open Settings › Devices › Link a new device and scan this code.")
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "It will show six digits to type here. The code works once, for ten minutes.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.height(12.dp))
                    TextButton(onClick = { offer = null; onCancel() }) { Text("Cancel") }
                }
            }
        }
        joined && link != null && link.stage != "code" -> {
            Text(endText(link, host = false))
            Spacer(Modifier.height(16.dp))
            if (link.stage != "done") {
                OutlinedButton(onClick = { joined = false; onCancel() }) { Text("Try again") }
            }
        }
        joined && link?.stage == "code" && link.code != null -> {
            Text("Check that your other device shows the same digits, then confirm there.")
            Digits(link.code)
            Text(
                "If they differ, cancel: someone else may have your code.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(12.dp))
            TextButton(onClick = { joined = false; onCancel() }) { Text("Cancel") }
        }
        joined -> {
            Text("Waiting for your other device…")
            Spacer(Modifier.height(12.dp))
            TextButton(onClick = { joined = false; onCancel() }) { Text("Cancel") }
        }
        else -> {
            Text(
                if (manual) "On your other device, open Settings › Devices › Link a new device, then choose \"Show a code instead\". Scan or type that code here."
                else "Show a code here and scan it with your other device (Settings › Devices › Link a new device).",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(16.dp))
            if (!manual) {
                Button(
                    enabled = !busy,
                    onClick = {
                        busy = true
                        error = null
                        onStartScanOffer(name) { r ->
                            busy = false
                            r.onSuccess { offer = it }.onFailure { error = it.message }
                        }
                    },
                    modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp),
                ) { Text("Show code to scan") }
            } else {
                Button(onClick = scan, modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp)) { Text("Scan the code") }
                Spacer(Modifier.height(12.dp))
                DuoTextField(
                    value = code,
                    onValueChange = { code = it.trim() },
                    label = { Text("Or type the code") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            Spacer(Modifier.height(8.dp))
            DuoTextField(
                value = name,
                onValueChange = { name = it.take(64) },
                label = { Text("Name for this device") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            error?.let {
                Spacer(Modifier.height(8.dp))
                Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
            }
            Spacer(Modifier.height(16.dp))
            WrapRow(8.dp) {
                OutlinedButton(onClick = onBack) { Text("Back") }
                TextButton(onClick = { manual = !manual }) { Text(if (manual) "Show a code instead" else "Scan their code instead") }
                if (manual) Button(
                    enabled = !busy && code.length >= 10,
                    onClick = {
                        busy = true
                        error = null
                        onJoin(code, name) { r ->
                            busy = false
                            r.onSuccess { joined = true }.onFailure { error = it.message }
                        }
                    },
                ) { Text("Link") }
            }
        }
    }
}

@Composable
private fun Digits(code: String) {
    Text(
        code.take(3) + " " + code.drop(3),
        fontFamily = FontFamily.Monospace,
        fontWeight = FontWeight.Bold,
        fontSize = 34.sp,
        letterSpacing = 4.sp,
        textAlign = TextAlign.Center,
        modifier = Modifier.fillMaxWidth().padding(vertical = 12.dp),
    )
}

private fun endText(link: DeviceLinkState?, host: Boolean): String = when (link?.stage) {
    "done" -> if (host) "${link.name ?: "The new device"} is linked." else "Linked. Your chats are on their way."
    "expired" -> "The code expired. Start again."
    "cancelled" -> "Linking was cancelled."
    else -> "Linking did not work. Start again."
}
