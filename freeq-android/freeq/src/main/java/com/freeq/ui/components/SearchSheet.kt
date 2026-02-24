package com.freeq.ui.components

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.freeq.model.AppState
import com.freeq.model.ChatMessage
import com.freeq.ui.theme.Theme
import java.text.SimpleDateFormat
import java.util.*

private data class SearchResult(
    val channel: String,
    val message: ChatMessage
)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SearchSheet(
    appState: AppState,
    onDismiss: () -> Unit,
    onNavigateToChannel: (channel: String, messageId: String) -> Unit
) {
    var query by remember { mutableStateOf("") }

    val results = remember(query) {
        if (query.length < 2) return@remember emptyList()
        val q = query.lowercase()
        val matches = mutableListOf<SearchResult>()
        for (channel in appState.channels + appState.dmBuffers) {
            for (msg in channel.messages) {
                if (msg.from.isEmpty() || msg.isDeleted) continue
                if (msg.text.lowercase().contains(q) || msg.from.lowercase().contains(q)) {
                    matches.add(SearchResult(channel = channel.name, message = msg))
                }
            }
        }
        matches.takeLast(50).reversed()
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .fillMaxHeight(0.85f)
                .padding(horizontal = 16.dp)
        ) {
            // Search input
            OutlinedTextField(
                value = query,
                onValueChange = { query = it },
                modifier = Modifier.fillMaxWidth(),
                placeholder = { Text("Search messages...", fontSize = 15.sp) },
                leadingIcon = {
                    Icon(
                        Icons.Default.Search,
                        contentDescription = null,
                        modifier = Modifier.size(20.dp)
                    )
                },
                singleLine = true,
                shape = RoundedCornerShape(12.dp),
                colors = OutlinedTextFieldDefaults.colors(
                    focusedBorderColor = MaterialTheme.colorScheme.primary,
                    unfocusedBorderColor = MaterialTheme.colorScheme.outline.copy(alpha = 0.3f),
                    focusedContainerColor = MaterialTheme.colorScheme.surfaceVariant,
                    unfocusedContainerColor = MaterialTheme.colorScheme.surfaceVariant,
                ),
                textStyle = LocalTextStyle.current.copy(fontSize = 15.sp)
            )

            Spacer(modifier = Modifier.height(12.dp))

            when {
                query.length < 2 -> {
                    Box(
                        modifier = Modifier.fillMaxSize(),
                        contentAlignment = Alignment.Center
                    ) {
                        Text(
                            "Type at least 2 characters to search",
                            fontSize = 14.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
                        )
                    }
                }
                results.isEmpty() -> {
                    Box(
                        modifier = Modifier.fillMaxSize(),
                        contentAlignment = Alignment.Center
                    ) {
                        Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            Icon(
                                Icons.Default.Search,
                                contentDescription = null,
                                modifier = Modifier.size(48.dp),
                                tint = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.4f)
                            )
                            Spacer(modifier = Modifier.height(12.dp))
                            Text(
                                "No results found",
                                fontSize = 16.sp,
                                color = MaterialTheme.colorScheme.onSurfaceVariant
                            )
                        }
                    }
                }
                else -> {
                    LazyColumn(
                        modifier = Modifier.fillMaxSize(),
                        verticalArrangement = Arrangement.spacedBy(2.dp)
                    ) {
                        items(results, key = { it.message.id }) { result ->
                            SearchResultRow(
                                result = result,
                                query = query,
                                onClick = {
                                    onNavigateToChannel(result.channel, result.message.id)
                                    onDismiss()
                                }
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun SearchResultRow(
    result: SearchResult,
    query: String,
    onClick: () -> Unit
) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(vertical = 10.dp, horizontal = 4.dp)
    ) {
        // Channel + timestamp
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically
        ) {
            Text(
                text = result.channel,
                fontSize = 12.sp,
                fontWeight = FontWeight.Medium,
                color = MaterialTheme.colorScheme.primary
            )
            Text(
                text = formatSearchTime(result.message.timestamp),
                fontSize = 11.sp,
                color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
            )
        }

        Spacer(modifier = Modifier.height(2.dp))

        // Nick + message with highlighted query
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(
                text = result.message.from,
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
                color = Theme.nickColor(result.message.from)
            )
            Text(
                text = highlightQuery(result.message.text, query),
                fontSize = 13.sp,
                color = MaterialTheme.colorScheme.onBackground,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f)
            )
        }

        Spacer(modifier = Modifier.height(8.dp))
        HorizontalDivider(color = MaterialTheme.colorScheme.outline.copy(alpha = 0.15f))
    }
}

private fun highlightQuery(text: String, query: String) = buildAnnotatedString {
    val lower = text.lowercase()
    val q = query.lowercase()
    var start = 0
    while (start < text.length) {
        val idx = lower.indexOf(q, start)
        if (idx < 0) {
            append(text.substring(start))
            break
        }
        append(text.substring(start, idx))
        pushStyle(SpanStyle(fontWeight = FontWeight.Bold))
        append(text.substring(idx, idx + q.length))
        pop()
        start = idx + q.length
    }
}

private fun formatSearchTime(date: Date): String {
    val cal = Calendar.getInstance()
    val today = Calendar.getInstance()
    cal.time = date

    return when {
        cal.get(Calendar.YEAR) == today.get(Calendar.YEAR) &&
                cal.get(Calendar.DAY_OF_YEAR) == today.get(Calendar.DAY_OF_YEAR) ->
            SimpleDateFormat("HH:mm", Locale.getDefault()).format(date)
        cal.get(Calendar.YEAR) == today.get(Calendar.YEAR) &&
                cal.get(Calendar.DAY_OF_YEAR) == today.get(Calendar.DAY_OF_YEAR) - 1 ->
            "Yesterday"
        else ->
            SimpleDateFormat("dd/MM/yy", Locale.getDefault()).format(date)
    }
}
