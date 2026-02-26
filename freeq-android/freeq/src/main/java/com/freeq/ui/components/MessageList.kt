package com.freeq.ui.components

import android.view.ContextThemeWrapper
import androidx.compose.foundation.background
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Reply
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalHapticFeedback
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
    onProfileClick: ((String) -> Unit)? = null,
    scrollToMessageId: String? = null,
    modifier: Modifier = Modifier
) {
    val messages = channelState.messages
    val listState = rememberLazyListState()
    val clipboardManager = LocalClipboardManager.current
    var lightboxUrl by remember { mutableStateOf<String?>(null) }
    var highlightedMessageId by remember { mutableStateOf<String?>(null) }
    var threadMessage by remember { mutableStateOf<ChatMessage?>(null) }

    // Snapshot last-read position from before this screen visit
    val lastReadId = remember(channelState.name) {
        appState.lastReadMessageIds[channelState.name]
    }
    val lastReadTimestamp = remember(channelState.name) {
        appState.lastReadTimestamps[channelState.name] ?: 0L
    }

    // Scroll to specific message (from search)
    LaunchedEffect(scrollToMessageId) {
        val targetId = scrollToMessageId ?: return@LaunchedEffect
        val idx = messages.indexOfFirst { it.id == targetId }
        if (idx >= 0) {
            listState.animateScrollToItem(idx)
            highlightedMessageId = targetId
        }
    }

    // Auto-scroll to bottom on new messages (skip if we just scrolled to a search result)
    LaunchedEffect(messages.size) {
        if (messages.isNotEmpty() && scrollToMessageId == null) {
            listState.animateScrollToItem(messages.size - 1)
        }
    }

    // Load older history when scrolled to top
    val firstVisibleIndex by remember { derivedStateOf { listState.firstVisibleItemIndex } }
    LaunchedEffect(firstVisibleIndex) {
        if (firstVisibleIndex == 0 && messages.isNotEmpty()) {
            val oldestId = messages.first().id
            val target = appState.activeChannel.value ?: return@LaunchedEffect
            appState.sendRaw("CHATHISTORY BEFORE $target msgid=$oldestId 50")
        }
    }

    Box(modifier = modifier.fillMaxSize()) {
        LazyColumn(
            state = listState,
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(vertical = 8.dp)
        ) {
            val unreadSeparatorMsgId = findUnreadBoundary(
                messages, lastReadId, lastReadTimestamp, appState.nick.value
            )

            itemsIndexed(messages, key = { _, msg -> msg.id }) { index, msg ->
                val prevMsg = if (index > 0) messages[index - 1] else null
                val currentDate = formatDate(msg.timestamp)
                val prevDate = prevMsg?.let { formatDate(it.timestamp) }
                val timeDiff = if (prevMsg != null) msg.timestamp.time - prevMsg.timestamp.time else Long.MAX_VALUE

                // Unread separator — show before the first unread message
                val showingUnread = msg.id == unreadSeparatorMsgId
                if (showingUnread) {
                    UnreadSeparator()
                }

                // Date separator (skip if unread separator already shown at this boundary)
                if (prevDate == null || currentDate != prevDate) {
                    if (!showingUnread) {
                        DateSeparator(currentDate)
                    }
                }

                // System message (join/part/kick — from is empty)
                if (msg.from.isEmpty()) {
                    SystemMessage(msg.text)
                    return@itemsIndexed
                }

                // Deleted message
                if (msg.isDeleted) {
                    DeletedMessage(msg.from)
                    return@itemsIndexed
                }

                // Show header if sender changes, >5 min gap, or after date/system/deleted boundary
                val showHeader = prevMsg == null
                    || msg.from != prevMsg.from
                    || prevMsg.from.isEmpty()
                    || prevMsg.isDeleted
                    || timeDiff > 5 * 60 * 1000
                    || currentDate != prevDate

                MessageBubble(
                    msg = msg,
                    showHeader = showHeader,
                    isHighlighted = msg.id == highlightedMessageId,
                    appState = appState,
                    channelState = channelState,
                    clipboardManager = clipboardManager,
                    onNickClick = onProfileClick,
                    onImageClick = { url -> lightboxUrl = url },
                    onThreadClick = { threadMsg -> threadMessage = threadMsg }
                )
            }

            // Typing indicator
            val typers = channelState.activeTypers
            if (typers.isNotEmpty()) {
                item {
                    TypingIndicator(typers)
                }
            }
        }

        // Image lightbox overlay
        lightboxUrl?.let { url ->
            ImageLightbox(url = url, onDismiss = { lightboxUrl = null })
        }
    }

    // Thread sheet
    threadMessage?.let { msg ->
        ThreadSheet(
            rootMessage = msg,
            channelState = channelState,
            appState = appState,
            onDismiss = { threadMessage = null }
        )
    }
}

@Composable
private fun MessageBubble(
    msg: ChatMessage,
    showHeader: Boolean,
    isHighlighted: Boolean = false,
    appState: AppState,
    channelState: ChannelState,
    clipboardManager: androidx.compose.ui.platform.ClipboardManager,
    onNickClick: ((String) -> Unit)? = null,
    onImageClick: ((String) -> Unit)? = null,
    onThreadClick: ((ChatMessage) -> Unit)? = null
) {
    var showMenu by remember { mutableStateOf(false) }
    var showEmojiPicker by remember { mutableStateOf(false) }
    val haptic = LocalHapticFeedback.current
    val isOwn = msg.from.equals(appState.nick.value, ignoreCase = true)
    val isMention = !isOwn && appState.nick.value.isNotEmpty() &&
            msg.text.contains(appState.nick.value, ignoreCase = true)

    val bgModifier = when {
        isHighlighted -> Modifier.background(FreeqColors.accent.copy(alpha = 0.12f))
        isMention -> Modifier.background(MaterialTheme.colorScheme.primary.copy(alpha = 0.08f))
        else -> Modifier
    }

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .then(bgModifier)
            .padding(
                start = 16.dp,
                end = 16.dp,
                top = if (showHeader) 8.dp else 1.dp
            )
    ) {
        // Reply context — tap to open thread view
        if (msg.replyTo != null) {
            val parentMsg = channelState.messages.firstOrNull { it.id == msg.replyTo }
            if (parentMsg != null) {
                Row(
                    modifier = Modifier
                        .padding(start = 48.dp, bottom = 2.dp)
                        .clickable { onThreadClick?.invoke(msg) },
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
                UserAvatar(
                    nick = msg.from,
                    size = 36.dp,
                    modifier = Modifier.clickable { onNickClick?.invoke(msg.from) }
                )
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
                            color = Theme.nickColor(msg.from),
                            modifier = Modifier.clickable { onNickClick?.invoke(msg.from) }
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
                                color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.5f)
                            )
                        }
                    }
                }

                // Message text + inline embeds
                MessageContent(
                    text = msg.text,
                    isAction = msg.isAction,
                    fromNick = msg.from,
                    onImageClick = onImageClick
                )

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
                                    haptic.performHapticFeedback(HapticFeedbackType.LongPress)
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
            // Quick-react emoji row
            Row(
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
                horizontalArrangement = Arrangement.spacedBy(4.dp)
            ) {
                listOf("\uD83D\uDC4D", "\u2764\uFE0F", "\uD83D\uDE02", "\uD83D\uDE2E", "\uD83D\uDE22", "\uD83D\uDC4E").forEach { emoji ->
                    Surface(
                        shape = RoundedCornerShape(8.dp),
                        color = MaterialTheme.colorScheme.surfaceVariant,
                        modifier = Modifier.clickable {
                            haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                            appState.activeChannel.value?.let { target ->
                                appState.sendReaction(target, msg.id, emoji)
                            }
                            showMenu = false
                        }
                    ) {
                        Text(
                            emoji,
                            fontSize = 20.sp,
                            modifier = Modifier.padding(8.dp)
                        )
                    }
                }
            }
            HorizontalDivider(modifier = Modifier.padding(vertical = 4.dp))
            DropdownMenuItem(
                text = { Text("Reply") },
                onClick = {
                    appState.replyingTo.value = msg
                    showMenu = false
                },
                leadingIcon = { Icon(Icons.AutoMirrored.Filled.Reply, contentDescription = null) }
            )
            val hasThread = msg.replyTo != null ||
                channelState.messages.any { it.replyTo == msg.id }
            if (hasThread) {
                DropdownMenuItem(
                    text = { Text("View Thread") },
                    onClick = {
                        onThreadClick?.invoke(msg)
                        showMenu = false
                    },
                    leadingIcon = { Icon(Icons.Default.Forum, contentDescription = null) }
                )
            }
            DropdownMenuItem(
                text = { Text("Copy") },
                onClick = {
                    clipboardManager.setText(AnnotatedString(msg.text))
                    showMenu = false
                },
                leadingIcon = { Icon(Icons.Default.ContentCopy, contentDescription = null) }
            )
            DropdownMenuItem(
                text = { Text("Add Reaction") },
                onClick = {
                    showMenu = false
                    showEmojiPicker = true
                },
                leadingIcon = { Icon(Icons.Default.AddReaction, contentDescription = null) }
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
                        haptic.performHapticFeedback(HapticFeedbackType.LongPress)
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

        // Emoji picker dialog
        if (showEmojiPicker) {
            AlertDialog(
                onDismissRequest = { showEmojiPicker = false },
                confirmButton = {},
                title = { Text("Add Reaction") },
                containerColor = MaterialTheme.colorScheme.surface,
                text = {
                    AndroidView(
                        factory = { context ->
                            val darkContext = ContextThemeWrapper(
                                context,
                                android.R.style.Theme_DeviceDefault
                            )
                            androidx.emoji2.emojipicker.EmojiPickerView(darkContext).apply {
                                setOnEmojiPickedListener { emojiViewItem ->
                                    appState.activeChannel.value?.let { target ->
                                        appState.sendReaction(target, msg.id, emojiViewItem.emoji)
                                    }
                                    showEmojiPicker = false
                                }
                            }
                        },
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(350.dp)
                    )
                }
            )
        }
    }
}

@Composable
private fun UnreadSeparator() {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 8.dp),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically
    ) {
        HorizontalDivider(
            modifier = Modifier.weight(1f),
            color = FreeqColors.accent
        )
        Text(
            text = "New messages",
            modifier = Modifier.padding(horizontal = 12.dp),
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            color = FreeqColors.accent
        )
        HorizontalDivider(
            modifier = Modifier.weight(1f),
            color = FreeqColors.accent
        )
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
private fun DeletedMessage(from: String) {
    Text(
        text = "Message from $from deleted",
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

/**
 * Find the first unread message ID to place the "New messages" separator before.
 * Tries matching by message ID first, falls back to timestamp for cross-session reliability.
 * Returns null if there are no unread messages or the user has already sent a message.
 */
private fun findUnreadBoundary(
    messages: List<ChatMessage>,
    lastReadId: String?,
    lastReadTimestamp: Long,
    nick: String
): String? {
    // Primary: find lastReadId in messages
    if (lastReadId != null) {
        val idx = messages.indexOfFirst { it.id == lastReadId }
        if (idx >= 0 && idx < messages.size - 1) {
            val tail = messages.subList(idx + 1, messages.size)
            val hasRealUnread = tail.any { it.from.isNotEmpty() }
            val userCaughtUp = tail.any { it.from.equals(nick, ignoreCase = true) }
            if (hasRealUnread && !userCaughtUp) return messages[idx + 1].id
        }
    }

    // Fallback: find first real message after lastReadTimestamp
    if (lastReadTimestamp > 0) {
        val idx = messages.indexOfFirst {
            it.timestamp.time > lastReadTimestamp && it.from.isNotEmpty()
        }
        if (idx >= 0) {
            val tail = messages.subList(idx, messages.size)
            val userCaughtUp = tail.any { it.from.equals(nick, ignoreCase = true) }
            if (!userCaughtUp) return messages[idx].id
        }
    }

    return null
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
