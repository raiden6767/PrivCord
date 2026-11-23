package com.duochat.app.ui

import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.duochat.app.AppUpdater
import com.duochat.app.UpdateDownload
import com.duochat.app.UpdateService
import com.duochat.app.UpdateSignal
import com.duochat.app.data.AppUpdate
import com.duochat.app.data.NetworkConfig
import com.duochat.app.data.UpdateInfo
import kotlinx.coroutines.launch

/** "Install" for a found update: downloads it, then Android asks the user to confirm. */
@Composable
fun UpdateInstallButton(info: UpdateInfo, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val download by UpdateSignal.download
    var message by remember { mutableStateOf<String?>(null) }
    val busy = download.busy
    val percent = download.percent
    Column(modifier) {
        if (busy) {
            DownloadPill(percent)
        } else Button(
            modifier = Modifier.fillMaxWidth(),
            onClick = {
                if (!context.packageManager.canRequestPackageInstalls()) {
                    message = "Allow installing apps from PrivCord on the next screen, then come back and tap Install again."
                    context.startActivity(
                        Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${context.packageName}"))
                            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                    )
                } else {
                    message = null
                    UpdateSignal.download.value = UpdateDownload(busy = true)
                    runCatching { UpdateService.start(context, info) }.onFailure {
                        UpdateSignal.download.value = UpdateDownload(message = "Could not start the download. Try again.")
                    }
                }
            },
        ) { Text("Install ${info.versionName}") }
        (message ?: download.message)?.let {
            Spacer(Modifier.height(6.dp))
            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** The Install button while downloading: same size, the colour fills from the left as the download advances. */
@Composable
private fun DownloadPill(percent: Int) {
    val progress by animateFloatAsState(percent.coerceIn(0, 100) / 100f, label = "download")
    val shape = RoundedCornerShape(50)
    Box(
        Modifier
            .fillMaxWidth()
            .height(40.dp)
            .clip(shape)
            .background(MaterialTheme.colorScheme.primary.copy(alpha = 0.25f))
            .semantics { progressBarRangeInfo = ProgressBarRangeInfo(progress, 0f..1f) },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.fillMaxHeight().fillMaxWidth(progress).background(MaterialTheme.colorScheme.primary).align(Alignment.CenterStart))
        Text(
            "Downloading… $percent%",
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.onSurface,
        )
    }
}

/** The prompt shown at start-up when a newer build exists. "Later" is remembered for a few days by the caller. */
@Composable
fun UpdateDialog(info: UpdateInfo, onLater: () -> Unit) {
    AlertDialog(
        onDismissRequest = onLater,
        title = { Text("Update available") },
        text = {
            Column {
                Text("PrivCord ${info.versionName} is ready. Your messages and settings stay as they are.")
                Spacer(Modifier.height(12.dp))
                UpdateInstallButton(info)
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onLater) { Text("Later") } },
    )
}

/** Settings: "Check now", with the result and an Install button. Blocked while Hide my IP or Tor is on. */
@Composable
fun UpdateCheckRow(network: NetworkConfig) {
    val scope = rememberCoroutineScope()
    var checking by remember { mutableStateOf(false) }
    var status by remember { mutableStateOf<String?>(null) }
    var found by remember { mutableStateOf<UpdateInfo?>(null) }
    Column {
        if (!AppUpdate.mayCheck(network)) {
            Text(
                "Turn off Hide my IP and Tor mode to check for updates: a check contacts GitHub directly.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        } else {
            OutlinedButton(
                enabled = !checking,
                onClick = {
                    checking = true
                    status = null
                    found = null
                    scope.launch {
                        AppUpdater.latest()
                            .onSuccess { info -> found = info; status = if (info == null) "You are up to date." else "Version ${info.versionName} is available." }
                            .onFailure { status = "Could not check for updates. Try again later." }
                        checking = false
                    }
                },
            ) { Text(if (checking) "Checking…" else "Check for updates now") }
        }
        status?.let {
            Spacer(Modifier.height(6.dp))
            Text(it, style = MaterialTheme.typography.bodySmall)
        }
        found?.let {
            Spacer(Modifier.height(8.dp))
            UpdateInstallButton(it)
        }
    }
}
