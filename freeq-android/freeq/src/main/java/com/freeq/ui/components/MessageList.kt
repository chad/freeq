package com.freeq.ui.components

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Reply
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.freeq.model.AppState
import com.freeq.model.ChannelState
import com.freeq.model.ChatMessage
import com.freeq.ui.theme.FreeqColors
import com.freeq.ui.theme.Theme
import java.text.SimpleDateFormat
import java.util.*

@Composable
fun MessageList(
    appState: AppState,
    channelState: ChannelState,
    modifier: Modifier = Modifier
) {
    val messages = channelState.messages
    val listState = rememberLazyListState()
    val clipboardManager = LocalClipboardManager.current

    // Auto-scroll to bottom on new messages
    LaunchedEffect(messages.size) {
        if (messages.isNotEmpty()) {
            listState.animateScrollToItem(messages.size - 1)
        }
    }

    LazyColumn(
        state = listState,
        modifier = modifier.fillMaxSize(),
        contentPadding = PaddingValues(vertical = 8.dp)
    ) {
        var lastSender = ""
        var lastDate = ""
        var lastTimestamp = 0L

        items(messages, key = { it.id }) { msg ->
            val currentDate = formatDate(msg.timestamp)
            val timeDiff = msg.timestamp.time - lastTimestamp

            // Date separator
            if (currentDate != lastDate) {
                DateSeparator(currentDate)
                lastDate = currentDate
                lastSender = "" // reset grouping after date
            }

            // System message (join/part/kick — from is empty)
            if (msg.from.isEmpty()) {
                SystemMessage(msg.text)
                lastSender = ""
                lastTimestamp = msg.timestamp.time
                return@items
            }

            // Deleted message
            if (msg.isDeleted) {
                DeletedMessage()
                lastSender = ""
                lastTimestamp = msg.timestamp.time
                return@items
            }

            // Show header if sender changes or >5 min gap
            val showHeader = msg.from != lastSender || timeDiff > 5 * 60 * 1000

            MessageBubble(
                msg = msg,
                showHeader = showHeader,
                appState = appState,
                channelState = channelState,
                clipboardManager = clipboardManager
            )

            lastSender = msg.from
            lastTimestamp = msg.timestamp.time
        }

        // Typing indicator
        val typers = channelState.activeTypers
        if (typers.isNotEmpty()) {
            item {
                TypingIndicator(typers)
            }
        }
    }
}

@Composable
private fun MessageBubble(
    msg: ChatMessage,
    showHeader: Boolean,
    appState: AppState,
    channelState: ChannelState,
    clipboardManager: androidx.compose.ui.platform.ClipboardManager
) {
    var showMenu by remember { mutableStateOf(false) }
    val isOwn = msg.from.equals(appState.nick.value, ignoreCase = true)

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(
                start = 16.dp,
                end = 16.dp,
                top = if (showHeader) 8.dp else 1.dp
            )
    ) {
        // Reply context
        if (msg.replyTo != null) {
            val parentMsg = channelState.messages.firstOrNull { it.id == msg.replyTo }
            if (parentMsg != null) {
                Row(
                    modifier = Modifier
                        .padding(start = 48.dp, bottom = 2.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(4.dp)
                ) {
                    Box(
                        modifier = Modifier
                            .width(2.dp)
                            .height(16.dp)
                            .background(MaterialTheme.colorScheme.primary)
                    )
                    Text(
                        text = "${parentMsg.from}: ${parentMsg.text}",
                        fontSize = 12.sp,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(start = 4.dp)
                    )
                }
            }
        }

        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            // Avatar (only on header rows)
            if (showHeader) {
                UserAvatar(nick = msg.from, size = 36.dp)
            } else {
                Spacer(modifier = Modifier.width(36.dp))
            }

            Column(
                modifier = Modifier
                    .weight(1f)
                    .clickable { showMenu = true }
            ) {
                // Header: nick + time
                if (showHeader) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp)
                    ) {
                        Text(
                            text = msg.from,
                            fontSize = 14.sp,
                            fontWeight = FontWeight.SemiBold,
                            color = Theme.nickColor(msg.from)
                        )
                        Text(
                            text = formatTime(msg.timestamp),
                            fontSize = 11.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
                        )
                        if (msg.isEdited) {
                            Text(
                                text = "(edited)",
                                fontSize = 11.sp,
                                fontStyle = FontStyle.Italic,
                                color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.5f)
                            )
                        }
                    }
                }

                // Message text
                if (msg.isAction) {
                    Text(
                        text = "${msg.from} ${msg.text}",
                        fontSize = 15.sp,
                        fontStyle = FontStyle.Italic,
                        color = MaterialTheme.colorScheme.onBackground
                    )
                } else {
                    Text(
                        text = msg.text,
                        fontSize = 15.sp,
                        color = MaterialTheme.colorScheme.onBackground
                    )
                }

                // Reactions
                if (msg.reactions.isNotEmpty()) {
                    Row(
                        modifier = Modifier.padding(top = 4.dp),
                        horizontalArrangement = Arrangement.spacedBy(4.dp)
                    ) {
                        msg.reactions.forEach { (emoji, nicks) ->
                            val isSelfReacted = nicks.any {
                                it.equals(appState.nick.value, ignoreCase = true)
                            }
                            Surface(
                                shape = RoundedCornerShape(12.dp),
                                color = if (isSelfReacted)
                                    MaterialTheme.colorScheme.primary.copy(alpha = 0.2f)
                                else
                                    MaterialTheme.colorScheme.surfaceVariant,
                                modifier = Modifier.clickable {
                                    appState.activeChannel.value?.let { target ->
                                        appState.sendReaction(target, msg.id, emoji)
                                    }
                                }
                            ) {
                                Row(
                                    modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
                                    horizontalArrangement = Arrangement.spacedBy(4.dp),
                                    verticalAlignment = Alignment.CenterVertically
                                ) {
                                    Text(emoji, fontSize = 14.sp)
                                    Text(
                                        "${nicks.size}",
                                        fontSize = 12.sp,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }

        // Context menu
        DropdownMenu(
            expanded = showMenu,
            onDismissRequest = { showMenu = false }
        ) {
            DropdownMenuItem(
                text = { Text("Reply") },
                onClick = {
                    appState.replyingTo.value = msg
                    showMenu = false
                },
                leadingIcon = { Icon(Icons.AutoMirrored.Filled.Reply, contentDescription = null) }
            )
            DropdownMenuItem(
                text = { Text("Copy") },
                onClick = {
                    clipboardManager.setText(AnnotatedString(msg.text))
                    showMenu = false
                },
                leadingIcon = { Icon(Icons.Default.ContentCopy, contentDescription = null) }
            )
            if (isOwn) {
                DropdownMenuItem(
                    text = { Text("Edit") },
                    onClick = {
                        appState.editingMessage.value = msg
                        showMenu = false
                    },
                    leadingIcon = { Icon(Icons.Default.Edit, contentDescription = null) }
                )
                DropdownMenuItem(
                    text = { Text("Delete") },
                    onClick = {
                        appState.activeChannel.value?.let { target ->
                            appState.deleteMessage(target, msg.id)
                        }
                        showMenu = false
                    },
                    leadingIcon = {
                        Icon(
                            Icons.Default.Delete,
                            contentDescription = null,
                            tint = FreeqColors.danger
                        )
                    }
                )
            }
        }
    }
}

@Composable
private fun DateSeparator(date: String) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 12.dp),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically
    ) {
        HorizontalDivider(
            modifier = Modifier.weight(1f),
            color = MaterialTheme.colorScheme.outline.copy(alpha = 0.3f)
        )
        Text(
            text = date,
            modifier = Modifier.padding(horizontal = 12.dp),
            fontSize = 12.sp,
            fontWeight = FontWeight.Medium,
            color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
        )
        HorizontalDivider(
            modifier = Modifier.weight(1f),
            color = MaterialTheme.colorScheme.outline.copy(alpha = 0.3f)
        )
    }
}

@Composable
private fun SystemMessage(text: String) {
    Text(
        text = text,
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 4.dp),
        fontSize = 12.sp,
        color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f),
        fontStyle = FontStyle.Italic
    )
}

@Composable
private fun DeletedMessage() {
    Text(
        text = "Message deleted",
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 64.dp, vertical = 2.dp),
        fontSize = 13.sp,
        color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.4f),
        fontStyle = FontStyle.Italic
    )
}

@Composable
private fun TypingIndicator(typers: List<String>) {
    val text = when {
        typers.size == 1 -> "${typers[0]} is typing..."
        typers.size == 2 -> "${typers[0]} and ${typers[1]} are typing..."
        else -> "${typers[0]} and ${typers.size - 1} others are typing..."
    }

    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp)
    ) {
        // Animated dots
        Text(
            text = "...",
            fontSize = 16.sp,
            fontWeight = FontWeight.Bold,
            color = MaterialTheme.colorScheme.primary
        )
        Text(
            text = text,
            fontSize = 12.sp,
            fontStyle = FontStyle.Italic,
            color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
        )
    }
}

private fun formatTime(date: Date): String {
    return SimpleDateFormat("HH:mm", Locale.getDefault()).format(date)
}

private fun formatDate(date: Date): String {
    val cal = Calendar.getInstance()
    val today = Calendar.getInstance()
    cal.time = date

    return when {
        cal.get(Calendar.YEAR) == today.get(Calendar.YEAR) &&
                cal.get(Calendar.DAY_OF_YEAR) == today.get(Calendar.DAY_OF_YEAR) -> "Today"
        cal.get(Calendar.YEAR) == today.get(Calendar.YEAR) &&
                cal.get(Calendar.DAY_OF_YEAR) == today.get(Calendar.DAY_OF_YEAR) - 1 -> "Yesterday"
        else -> SimpleDateFormat("MMMM d, yyyy", Locale.getDefault()).format(date)
    }
}
