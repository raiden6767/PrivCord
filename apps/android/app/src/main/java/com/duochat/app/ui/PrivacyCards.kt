package com.duochat.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.duochat.app.data.CheckupItem
import com.duochat.app.data.Health
import com.duochat.app.data.HostRow

/** A short list of privacy basics, how many hold, and a one-tap "Turn on" where the phone can do it itself. */
@Composable
fun PrivacyCheckupCard(items: List<CheckupItem>, onTurnOn: (String) -> Unit, canTurnOn: Set<String>) {
    val score = items.count { it.ok }
    Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceContainer, modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Privacy checkup", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                Text("$score/${items.size}", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            }
            Spacer(Modifier.height(8.dp))
            LinearProgressIndicator(
                progress = { score / items.size.toFloat() },
                modifier = Modifier.fillMaxWidth().height(6.dp),
                color = MaterialTheme.colorScheme.primary,
                trackColor = MaterialTheme.colorScheme.surfaceContainerHighest,
                strokeCap = androidx.compose.ui.graphics.StrokeCap.Round,
                drawStopIndicator = {},
            )
            Spacer(Modifier.height(8.dp))
            items.forEach { item ->
                Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalAlignment = Alignment.Top) {
                    Icon(
                        if (item.ok) Icons.Filled.CheckCircle else Icons.Filled.Warning,
                        contentDescription = if (item.ok) "In place" else "Needs attention",
                        tint = if (item.ok) com.duochat.app.ui.Online else MaterialTheme.colorScheme.tertiary,
                        modifier = Modifier.size(20.dp),
                    )
                    Spacer(Modifier.width(10.dp))
                    Column(Modifier.weight(1f)) {
                        Text(item.label, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
                        Text(item.hint, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    if (!item.ok && item.id in canTurnOn) TextButton(onClick = { onTurnOn(item.id) }) { Text("Turn on") }
                }
            }
        }
    }
}

/** Every internet host this phone may contact, and why, straight from the settings that decide it. */
@Composable
fun NetworkHostsCard(rows: List<HostRow>) {
    Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceContainer, modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("What this app connects to", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            if (rows.isEmpty()) {
                Text("Nothing. In this mode PrivCord only uses Bluetooth and the local network.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            } else {
                rows.forEach { r ->
                    Column {
                        SelectionContainer { Text(r.host, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodyMedium) }
                        Text(r.why, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
            Text(
                "That is the whole list: no analytics, no ads, no account server. The one other host is github.com, only to look for a " +
                    "new version at start (Settings: you can turn that off, and it never runs with Hide my IP or Tor on). Your messages " +
                    "go phone to phone; the hosts above only help two phones find each other, and see sealed data they cannot read.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

private fun formatSize(b: Long): String = when {
    b >= 1_000_000 -> "%.1f MB".format(b / 1_000_000.0)
    b >= 1_000 -> "%.0f KB".format(b / 1_000.0)
    else -> "$b B"
}

/** What each chat takes on this phone, with a way to drop a chat's photos and files to free space. */
@Composable
fun StorageByChatCard(rows: List<com.duochat.app.data.ChatStorage>, onDeleteFiles: (List<String>) -> Unit) {
    if (rows.isEmpty()) return
    var confirm by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf<String?>(null) }
    val biggest = rows.first().bytes.coerceAtLeast(1L)
    Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceContainer, modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text("Storage by chat", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            rows.take(12).forEach { r ->
                Column {
                    Row {
                        Text(r.name, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
                        Text(formatSize(r.bytes), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    Spacer(Modifier.height(4.dp))
                    LinearProgressIndicator(
                        progress = { (r.bytes / biggest.toFloat()).coerceIn(0.03f, 1f) },
                        modifier = Modifier.fillMaxWidth().height(6.dp),
                        color = MaterialTheme.colorScheme.primary,
                        trackColor = MaterialTheme.colorScheme.surfaceContainerHighest,
                        strokeCap = androidx.compose.ui.graphics.StrokeCap.Round,
                        drawStopIndicator = {},
                    )
                    if (r.files > 0) {
                        if (confirm == r.id) {
                            Row {
                                TextButton(onClick = { onDeleteFiles(r.fileIds); confirm = null }) {
                                    Text("Delete ${r.files} file message${if (r.files == 1) "" else "s"} for me", color = MaterialTheme.colorScheme.error)
                                }
                                TextButton(onClick = { confirm = null }) { Text("Keep") }
                            }
                        } else {
                            TextButton(onClick = { confirm = r.id }) { Text("Free ${formatSize(r.fileBytes)} of photos and files") }
                        }
                    }
                }
            }
            Text(
                "Deleting removes those messages from this phone only. Your friends keep their copies.",
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

/** A small "Sealed" chip for the chat's top bar; tapping it explains what protects this chat. */
@Composable
fun SealedChip(onClick: () -> Unit) {
    // Not Surface(onClick = ...): a tappable Surface is made at least 48 dp tall, which stretched the chat header and left a
    // big gap between the name and the status line. The chip stays its own small size; the tap still works.
    Surface(
        shape = RoundedCornerShape(50),
        color = MaterialTheme.colorScheme.secondaryContainer,
        contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
    ) {
        Row(
            Modifier.clickable(onClick = onClick).padding(horizontal = 7.dp, vertical = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Filled.Lock, contentDescription = null, modifier = Modifier.size(11.dp))
            Spacer(Modifier.width(3.dp))
            Text("Sealed", style = MaterialTheme.typography.labelSmall)
        }
    }
}

/** One calm line about the linked devices, as the engine words it. */
@Composable
fun DevicesStrip(line: String) {
    Surface(shape = RoundedCornerShape(14.dp), color = MaterialTheme.colorScheme.secondaryContainer, contentColor = MaterialTheme.colorScheme.onSecondaryContainer, modifier = Modifier.fillMaxWidth()) {
        Text(line, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(horizontal = 14.dp, vertical = 10.dp))
    }
}

/** The phone screens the engine can point at; "connection_settings" is on this same page, so it needs no button. */
private fun String.opensPhoneScreen(): Boolean = this == "notification_settings" || this == "battery_settings" || this == "background_settings"

/** What could stop messages arriving, with a one-tap fix, and a "Why am I not receiving?" answer. */
@Composable
fun DeliveryHealthCard(health: Health, onFix: (String) -> Unit) {
    var why by remember { mutableStateOf(false) }
    Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceContainer, modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp)) {
            Text("Delivery health", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.height(6.dp))
            health.items.forEach { item ->
                Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalAlignment = Alignment.Top) {
                    Icon(
                        if (item.ok) Icons.Filled.CheckCircle else Icons.Filled.Warning,
                        contentDescription = if (item.ok) "In place" else "Needs attention",
                        tint = if (item.ok) com.duochat.app.ui.Online else MaterialTheme.colorScheme.tertiary,
                        modifier = Modifier.size(20.dp),
                    )
                    Spacer(Modifier.width(10.dp))
                    Column(Modifier.weight(1f)) {
                        Text(item.title, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
                        if (!item.ok && item.hint.isNotBlank()) {
                            Text(item.hint, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                    if (!item.ok && item.fix.opensPhoneScreen()) TextButton(onClick = { onFix(item.fix) }) { Text("Fix") }
                }
            }
            health.verdict?.let { v ->
                Spacer(Modifier.height(4.dp))
                OutlinedButton(onClick = { why = !why }) { Text("Why am I not receiving?") }
                if (why) {
                    Spacer(Modifier.height(8.dp))
                    Text(v.text, style = MaterialTheme.typography.bodyMedium)
                    if (v.fix.opensPhoneScreen()) {
                        Spacer(Modifier.height(8.dp))
                        Button(onClick = { onFix(v.fix) }) { Text("Fix") }
                    }
                }
            }
        }
    }
}
