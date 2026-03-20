package com.freeq.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.PushPin
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.freeq.ui.theme.FreeqColors
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.net.URL
import java.text.SimpleDateFormat
import java.util.*

data class PinnedMessage(
    val id: String,
    val from: String,
    val text: String,
    val timestamp: Date,
    val pinnedBy: String
)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PinnedMessagesSheet(
    channelName: String,
    onDismiss: () -> Unit,
    onNavigateToMessage: ((String) -> Unit)? = null
) {
    var pins by remember { mutableStateOf<List<PinnedMessage>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(channelName) {
        loading = true
        error = null
        try {
            val result = withContext(Dispatchers.IO) { fetchPins(channelName) }
            pins = result
        } catch (e: Exception) {
            error = e.message ?: "Failed to load pins"
        }
        loading = false
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = MaterialTheme.colorScheme.background,
        dragHandle = { BottomSheetDefaults.DragHandle() }
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(bottom = 32.dp)
        ) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 24.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp)
            ) {
                Surface(
                    shape = RoundedCornerShape(12.dp),
                    color = FreeqColors.warning.copy(alpha = 0.15f),
                    modifier = Modifier.size(40.dp)
                ) {
                    Box(contentAlignment = Alignment.Center, modifier = Modifier.fillMaxSize()) {
                        Icon(
                            Icons.Default.PushPin,
                            contentDescription = null,
                            tint = FreeqColors.warning,
                            modifier = Modifier.size(20.dp)
                        )
                    }
                }
                Text(
                    "Pinned Messages",
                    fontSize = 17.sp,
                    fontWeight = FontWeight.Bold,
                    color = MaterialTheme.colorScheme.onBackground
                )
            }

            Spacer(modifier = Modifier.height(16.dp))

            when {
                loading -> {
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(200.dp),
                        contentAlignment = Alignment.Center
                    ) {
                        CircularProgressIndicator(color = FreeqColors.accent)
                    }
                }
                error != null -> {
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(200.dp),
                        contentAlignment = Alignment.Center
                    ) {
                        Text(error ?: "Error", color = FreeqColors.danger)
                    }
                }
                pins.isEmpty() -> {
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(200.dp),
                        contentAlignment = Alignment.Center
                    ) {
                        Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            Icon(
                                Icons.Default.PushPin,
                                contentDescription = null,
                                tint = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.4f),
                                modifier = Modifier.size(40.dp)
                            )
                            Spacer(modifier = Modifier.height(12.dp))
                            Text(
                                "No pinned messages",
                                fontSize = 16.sp,
                                fontWeight = FontWeight.Medium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant
                            )
                        }
                    }
                }
                else -> {
                    LazyColumn(
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(max = 400.dp)
                    ) {
                        items(pins, key = { it.id }) { pin ->
                            PinnedMessageRow(
                                pin = pin,
                                onClick = { onNavigateToMessage?.invoke(pin.id) }
                            )
                            HorizontalDivider(
                                modifier = Modifier.padding(start = 68.dp),
                                color = MaterialTheme.colorScheme.outline.copy(alpha = 0.2f)
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun PinnedMessageRow(pin: PinnedMessage, onClick: () -> Unit) {
    val timeFormat = remember { SimpleDateFormat("MMM d, h:mm a", Locale.getDefault()) }

    Surface(onClick = onClick, color = MaterialTheme.colorScheme.background) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 12.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp)
        ) {
            UserAvatar(nick = pin.from, size = 40.dp)

            Column(modifier = Modifier.weight(1f)) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp)
                ) {
                    Text(
                        pin.from,
                        fontSize = 14.sp,
                        fontWeight = FontWeight.SemiBold,
                        color = MaterialTheme.colorScheme.onBackground
                    )
                    Text(
                        timeFormat.format(pin.timestamp),
                        fontSize = 11.sp,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                }

                Spacer(modifier = Modifier.height(4.dp))

                Text(
                    pin.text,
                    fontSize = 14.sp,
                    color = MaterialTheme.colorScheme.onBackground,
                    maxLines = 3,
                    overflow = TextOverflow.Ellipsis
                )

                Spacer(modifier = Modifier.height(6.dp))

                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(4.dp)
                ) {
                    Icon(
                        Icons.Default.PushPin,
                        contentDescription = null,
                        tint = FreeqColors.warning,
                        modifier = Modifier.size(12.dp)
                    )
                    Text(
                        "Pinned by ${pin.pinnedBy}",
                        fontSize = 11.sp,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                }
            }
        }
    }
}

private fun fetchPins(channelName: String): List<PinnedMessage> {
    val encoded = java.net.URLEncoder.encode(channelName, "UTF-8")
    val url = URL("https://irc.freeq.at/api/v1/channels/$encoded/pins")
    val conn = url.openConnection() as java.net.HttpURLConnection
    conn.connectTimeout = 10_000
    conn.readTimeout = 10_000

    if (conn.responseCode != 200) {
        throw Exception("Failed to load pins (status ${conn.responseCode})")
    }

    val body = conn.inputStream.bufferedReader().readText()
    val json = JSONObject(body)
    val pinsArray = json.optJSONArray("pins") ?: return emptyList()

    val result = mutableListOf<PinnedMessage>()
    for (i in 0 until pinsArray.length()) {
        val pin = pinsArray.getJSONObject(i)
        val msgid = pin.optString("msgid", "")
        val fromRaw = pin.optString("from", "")
        val from = fromRaw.substringBefore("!")
        val text = pin.optString("text", "")
        if (msgid.isEmpty() || from.isEmpty()) continue

        val timestamp = try {
            val ts = pin.optString("timestamp", "")
            if (ts.isNotEmpty()) {
                SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss", Locale.US).parse(ts) ?: Date()
            } else Date()
        } catch (_: Exception) { Date() }

        val pinnedBy = pin.optString("pinned_by", "unknown")
        result.add(PinnedMessage(msgid, from, text, timestamp, pinnedBy))
    }
    return result
}
