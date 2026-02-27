package com.freeq.ui.components

import android.content.Context
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.freeq.model.AppState

@Composable
fun MotdDialog(appState: AppState) {
    if (!appState.showMotd.value) return

    AlertDialog(
        onDismissRequest = { dismissMotd(appState) },
        title = { Text("Message of the Day") },
        confirmButton = {
            TextButton(onClick = { dismissMotd(appState) }) {
                Text("OK")
            }
        },
        text = {
            LazyColumn(modifier = Modifier.heightIn(max = 400.dp)) {
                items(appState.motdLines.toList()) { line ->
                    if (line.isBlank()) {
                        Spacer(Modifier.height(8.dp))
                    } else {
                        Text(
                            text = line,
                            fontSize = 14.sp,
                            color = MaterialTheme.colorScheme.onSurface
                        )
                    }
                }
            }
        }
    )
}

private fun dismissMotd(appState: AppState) {
    appState.showMotd.value = false
    val content = appState.motdLines.joinToString("\n")
    val hash = content.hashCode().toString(36)
    appState.prefs.edit().putString("motd_seen_hash", hash).apply()
}
