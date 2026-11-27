package com.duochat.app.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.duochat.app.engine.Diag
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.zip.GZIPOutputStream

/**
 * Settings > Diagnostic log: a log of what the app and its screens did, kept on this phone only. It holds no message text,
 * names, keys or addresses. It is saved to a file the person chooses (nothing is uploaded) as Markdown, which can be sent
 * as it is, or compressed when it is long.
 */
@Composable
fun DiagLogSection(
    status: suspend () -> String,
    export: suspend (String) -> String,
    clear: suspend () -> Unit,
    setEnabled: suspend (Boolean) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var enabled by remember { mutableStateOf(true) }
    var entries by remember { mutableStateOf(0) }
    var message by remember { mutableStateOf<String?>(null) }
    var pending by remember { mutableStateOf<String?>(null) }
    var compressed by remember { mutableStateOf(false) }

    suspend fun refresh() {
        runCatching { JSONObject(status()) }.getOrNull()?.let {
            enabled = it.optBoolean("enabled", true)
            entries = it.optInt("entries", 0)
        }
    }
    LaunchedEffect(Unit) { refresh() }

    fun write(uri: Uri?, gzip: Boolean) {
        val text = pending
        pending = null
        if (uri == null || text == null) return
        scope.launch {
            val ok = withContext(Dispatchers.IO) {
                runCatching {
                    context.contentResolver.openOutputStream(uri)?.use { out ->
                        if (gzip) GZIPOutputStream(out).use { it.write(text.toByteArray(Charsets.UTF_8)) } else out.write(text.toByteArray(Charsets.UTF_8))
                    } ?: error("no stream")
                }.isSuccess
            }
            message = if (ok) "Log saved." else "Could not save the log."
            Diag.log("ui", "diagnostic log saved", "ok=$ok compressed=$gzip")
        }
    }
    val saveMd = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("text/markdown")) { write(it, false) }
    val saveGz = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/gzip")) { write(it, true) }

    fun stamp() = SimpleDateFormat("yyyyMMdd-HHmm", Locale.US).format(Date())

    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        ToggleRow(
            "Keep a diagnostic log on this phone",
            "Writes down what the app and its screens did (connections, messages going out, buttons pressed, errors) so a problem can be found. " +
                "No message text, names, keys or addresses go in it, and nothing is uploaded: it stays here until you save it. Turning this off empties it.",
            enabled,
        ) { on ->
            enabled = on
            scope.launch {
                setEnabled(on)
                refresh()
            }
        }
        Text(
            if (enabled) "$entries entries kept." else "Off.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        if (enabled) {
            OutlinedButton(onClick = {
                scope.launch {
                    Diag.log("ui", "tapped Save diagnostic log", "")
                    pending = export(Diag.about(context.applicationContext as android.app.Application))
                    compressed = false
                    saveMd.launch("privcord-log-${stamp()}.md")
                }
            }) { Text("Save the log (.md)") }
            OutlinedButton(onClick = {
                scope.launch {
                    Diag.log("ui", "tapped Save compressed diagnostic log", "")
                    pending = export(Diag.about(context.applicationContext as android.app.Application))
                    compressed = true
                    saveGz.launch("privcord-log-${stamp()}.md.gz")
                }
            }) { Text("Save it compressed (.md.gz)") }
            OutlinedButton(onClick = {
                scope.launch {
                    clear()
                    refresh()
                    message = "Log emptied."
                }
            }) { Text("Empty the log") }
        }
        message?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        Text(
            "To report a problem, save the log and send the file along with what went wrong and roughly when.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
