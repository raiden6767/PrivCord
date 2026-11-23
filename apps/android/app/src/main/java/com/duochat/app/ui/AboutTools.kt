package com.duochat.app.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.duochat.app.AppConfig
import com.duochat.app.data.NetworkConfig
import com.duochat.app.data.SelfTestRow
import com.duochat.app.data.SignalingConfig
import com.duochat.app.data.WhatsNew
import com.duochat.app.data.diagnosticsAppJson
import com.duochat.app.data.jsonString
import com.duochat.app.data.parseSelfTest
import com.duochat.app.data.parseWhatsNew
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Settings > About tools: what is new in this version, a counts-only diagnostics report saved to a file you choose, and
 * the engine's quick self-test. All texts and checks come from the shared engine.
 */
@Composable
fun AboutTools(
    network: NetworkConfig,
    insight: suspend (String, String) -> String,
    diagnosticsJson: suspend (String) -> String,
    selfTestJson: suspend () -> String,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val news by produceState<WhatsNew?>(null) { value = parseWhatsNew(insight("whats_new", jsonString(AppConfig.APP_VERSION))) }
    var pending by remember { mutableStateOf<String?>(null) }
    var status by remember { mutableStateOf<String?>(null) }
    var tests by remember { mutableStateOf<List<SelfTestRow>?>(null) }
    var running by remember { mutableStateOf(false) }
    val saver = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/json")) { uri: Uri? ->
        val text = pending
        pending = null
        if (uri != null && text != null) scope.launch {
            val ok = withContext(Dispatchers.IO) {
                runCatching { context.contentResolver.openOutputStream(uri)?.use { it.write(text.toByteArray()) } ?: error("no stream") }.isSuccess
            }
            status = if (ok) "Report saved." else "Could not save the report."
        }
    }
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        news?.let { n -> WhatsNewCard(n) }
        Text(
            "A diagnostics report has counts and settings only: no messages, names or keys. Share it if something is not working.",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        OutlinedButton(onClick = {
            scope.launch {
                val relays = network.signaling.sumOf { s -> if (s is SignalingConfig.Nostr) s.relays.size else 1 }
                val app = diagnosticsAppJson(AppConfig.APP_VERSION, android.os.Build.VERSION.SDK_INT, network.transportMode.name, relays, network.torMode)
                pending = diagnosticsJson(app)
                saver.launch("privcord-diagnostics.json")
            }
        }) { Text("Save a diagnostics report") }
        status?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        OutlinedButton(enabled = !running, onClick = {
            running = true
            scope.launch {
                tests = parseSelfTest(selfTestJson())
                running = false
            }
        }) { Text(if (running) "Running…" else "Run a self-test") }
        tests?.let { rows ->
            if (rows.isEmpty()) Text("The self-test could not run.", style = MaterialTheme.typography.bodySmall)
            rows.forEach { r ->
                Row(verticalAlignment = Alignment.Top) {
                    Icon(
                        if (r.ok) Icons.Filled.CheckCircle else Icons.Filled.Warning,
                        contentDescription = if (r.ok) "Passed" else "Failed",
                        tint = if (r.ok) Online else MaterialTheme.colorScheme.error,
                        modifier = Modifier.size(18.dp),
                    )
                    Spacer(Modifier.width(8.dp))
                    Column {
                        Text(r.name, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
                        if (r.detail.isNotBlank()) Text(r.detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
    }
}

/** The "What's new" lines for a version, as the engine words them. */
@Composable
fun WhatsNewCard(news: WhatsNew) {
    Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceContainer, modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text("What's new", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.height(2.dp))
            news.lines.forEach { Text("\u2022 $it", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        }
    }
}
