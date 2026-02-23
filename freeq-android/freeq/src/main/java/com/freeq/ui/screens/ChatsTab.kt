package com.freeq.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Chat
import androidx.compose.material.icons.automirrored.filled.ExitToApp
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.freeq.model.AppState
import com.freeq.model.ChannelState
import com.freeq.model.ChatMessage
import com.freeq.ui.components.UserAvatar
import com.freeq.ui.theme.FreeqColors
import com.freeq.ui.theme.Theme
import java.text.SimpleDateFormat
import java.util.*

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChatsTab(
    appState: AppState,
    onChannelClick: (String) -> Unit
) {
    var searchText by remember { mutableStateOf("") }
    var showJoinDialog by remember { mutableStateOf(false) }

    val allConversations = appState.channels + appState.dmBuffers
    val filteredConversations = if (searchText.isEmpty()) {
        allConversations
    } else {
        allConversations.filter { it.name.contains(searchText, ignoreCase = true) }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Chats") },
                actions = {
                    IconButton(onClick = { showJoinDialog = true }) {
                        Icon(
                            Icons.Default.Edit,
                            contentDescription = "New chat",
                            tint = MaterialTheme.colorScheme.primary
                        )
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.surface,
                    titleContentColor = MaterialTheme.colorScheme.onSurface
                )
            )
        }
    ) { padding ->
        Column(modifier = Modifier.padding(padding)) {
            // Network warning banner
            val networkConnected by appState.networkMonitor.isConnected
            if (!networkConnected) {
                Surface(
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.fillMaxWidth()
                ) {
                    Row(
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp)
                    ) {
                        Icon(
                            Icons.Default.WifiOff,
                            contentDescription = null,
                            modifier = Modifier.size(16.dp),
                            tint = MaterialTheme.colorScheme.onError
                        )
                        Text(
                            "No network connection",
                            fontSize = 13.sp,
                            color = MaterialTheme.colorScheme.onError
                        )
                    }
                }
            }

            // Search bar
            OutlinedTextField(
                value = searchText,
                onValueChange = { searchText = it },
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp, vertical = 8.dp),
                placeholder = { Text("Search chats") },
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
                    unfocusedBorderColor = MaterialTheme.colorScheme.outline,
                    focusedContainerColor = MaterialTheme.colorScheme.surfaceVariant,
                    unfocusedContainerColor = MaterialTheme.colorScheme.surfaceVariant,
                )
            )

            if (allConversations.isEmpty()) {
                // Empty state
                Box(
                    modifier = Modifier.fillMaxSize(),
                    contentAlignment = Alignment.Center
                ) {
                    Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(16.dp)
                    ) {
                        Icon(
                            Icons.AutoMirrored.Filled.Chat,
                            contentDescription = null,
                            modifier = Modifier.size(48.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.5f)
                        )
                        Text(
                            "No conversations yet",
                            fontSize = 18.sp,
                            fontWeight = FontWeight.Medium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant
                        )
                        Text(
                            "Join a channel to get started",
                            fontSize = 14.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.7f)
                        )
                        TextButton(onClick = { showJoinDialog = true }) {
                            Icon(
                                Icons.Default.Add,
                                contentDescription = null,
                                modifier = Modifier.size(18.dp)
                            )
                            Spacer(modifier = Modifier.width(4.dp))
                            Text("Join Channel")
                        }
                    }
                }
            } else {
                LazyColumn(modifier = Modifier.fillMaxSize()) {
                    items(filteredConversations, key = { it.name }) { conversation ->
                        val isChannel = conversation.name.startsWith("#")
                        if (isChannel) {
                            val dismissState = rememberSwipeToDismissBoxState(
                                confirmValueChange = { value ->
                                    if (value == SwipeToDismissBoxValue.EndToStart) {
                                        appState.partChannel(conversation.name)
                                        true
                                    } else false
                                }
                            )
                            SwipeToDismissBox(
                                state = dismissState,
                                backgroundContent = {
                                    Box(
                                        modifier = Modifier
                                            .fillMaxSize()
                                            .background(MaterialTheme.colorScheme.error)
                                            .padding(horizontal = 20.dp),
                                        contentAlignment = Alignment.CenterEnd
                                    ) {
                                        Icon(
                                            Icons.AutoMirrored.Filled.ExitToApp,
                                            contentDescription = "Leave",
                                            tint = MaterialTheme.colorScheme.onError
                                        )
                                    }
                                },
                                enableDismissFromStartToEnd = false
                            ) {
                                Surface(color = MaterialTheme.colorScheme.background) {
                                    ChatRow(
                                        conversation = conversation,
                                        unreadCount = appState.unreadCounts[conversation.name] ?: 0,
                                        onClick = { onChannelClick(conversation.name) }
                                    )
                                }
                            }
                        } else {
                            ChatRow(
                                conversation = conversation,
                                unreadCount = appState.unreadCounts[conversation.name] ?: 0,
                                onClick = { onChannelClick(conversation.name) }
                            )
                        }
                        HorizontalDivider(
                            modifier = Modifier.padding(start = 76.dp),
                            color = MaterialTheme.colorScheme.outline.copy(alpha = 0.3f)
                        )
                    }
                }
            }
        }
    }

    // Join channel dialog
    if (showJoinDialog) {
        JoinChannelDialog(
            appState = appState,
            onDismiss = { showJoinDialog = false }
        )
    }
}

@Composable
private fun ChatRow(
    conversation: ChannelState,
    unreadCount: Int,
    onClick: () -> Unit
) {
    val isChannel = conversation.name.startsWith("#")
    val lastMessage = conversation.messages.lastOrNull { it.from.isNotEmpty() && !it.isDeleted }
    val timeString = lastMessage?.let { formatTime(it.timestamp) } ?: ""
    val typingActive = conversation.activeTypers.isNotEmpty()

    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp)
    ) {
        // Avatar / channel icon
        if (isChannel) {
            Box(
                modifier = Modifier
                    .size(50.dp)
                    .clip(CircleShape)
                    .background(MaterialTheme.colorScheme.primary.copy(alpha = 0.15f)),
                contentAlignment = Alignment.Center
            ) {
                Text(
                    "#",
                    fontSize = 22.sp,
                    fontWeight = FontWeight.Bold,
                    color = MaterialTheme.colorScheme.primary
                )
            }
        } else {
            UserAvatar(nick = conversation.name, size = 50.dp)
        }

        // Content
        Column(modifier = Modifier.weight(1f)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Text(
                    text = conversation.name,
                    fontSize = 16.sp,
                    fontWeight = if (unreadCount > 0) FontWeight.Bold else FontWeight.Normal,
                    color = MaterialTheme.colorScheme.onBackground,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f)
                )
                Text(
                    text = timeString,
                    fontSize = 12.sp,
                    color = if (unreadCount > 0) MaterialTheme.colorScheme.primary
                    else MaterialTheme.colorScheme.onSurfaceVariant
                )
            }

            Spacer(modifier = Modifier.height(2.dp))

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                val previewText = when {
                    lastMessage != null -> {
                        if (lastMessage.isAction) "${lastMessage.from} ${lastMessage.text}"
                        else "${lastMessage.from}: ${lastMessage.text}"
                    }
                    conversation.topic.value.isNotEmpty() -> conversation.topic.value
                    isChannel -> "No messages yet"
                    else -> "Start a conversation"
                }

                Text(
                    text = previewText,
                    fontSize = 14.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f)
                )

                Row(
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    if (typingActive) {
                        Icon(
                            Icons.Default.MoreHoriz,
                            contentDescription = "Typing",
                            modifier = Modifier.size(16.dp),
                            tint = MaterialTheme.colorScheme.primary
                        )
                    }

                    if (unreadCount > 0) {
                        Box(
                            modifier = Modifier
                                .clip(CircleShape)
                                .background(MaterialTheme.colorScheme.primary)
                                .padding(horizontal = 7.dp, vertical = 2.dp),
                            contentAlignment = Alignment.Center
                        ) {
                            Text(
                                text = "$unreadCount",
                                fontSize = 12.sp,
                                fontWeight = FontWeight.Bold,
                                color = MaterialTheme.colorScheme.onPrimary
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun JoinChannelDialog(
    appState: AppState,
    onDismiss: () -> Unit
) {
    var channelName by remember { mutableStateOf("") }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Join Channel") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                OutlinedTextField(
                    value = channelName,
                    onValueChange = { channelName = it },
                    modifier = Modifier.fillMaxWidth(),
                    placeholder = { Text("channel-name") },
                    prefix = { Text("#") },
                    singleLine = true,
                    shape = RoundedCornerShape(10.dp)
                )

                Text(
                    "Popular channels",
                    fontSize = 12.sp,
                    fontWeight = FontWeight.Bold,
                    color = MaterialTheme.colorScheme.onSurfaceVariant
                )

                val popularChannels = listOf("#general", "#freeq", "#dev", "#music", "#random", "#crypto", "#gaming")
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    popularChannels.forEach { ch ->
                        val isJoined = appState.channels.any { it.name.equals(ch, ignoreCase = true) }
                        TextButton(
                            onClick = {
                                if (!isJoined) {
                                    appState.joinChannel(ch)
                                    onDismiss()
                                }
                            },
                            enabled = !isJoined
                        ) {
                            Text(
                                ch,
                                color = if (isJoined) MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.5f)
                                else MaterialTheme.colorScheme.primary
                            )
                            if (isJoined) {
                                Spacer(modifier = Modifier.width(4.dp))
                                Text("Joined", fontSize = 12.sp, color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.5f))
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = {
                    if (channelName.isNotEmpty()) {
                        appState.joinChannel(channelName)
                        onDismiss()
                    }
                },
                enabled = channelName.isNotEmpty()
            ) {
                Text("Join")
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text("Cancel")
            }
        }
    )
}

private fun formatTime(date: Date): String {
    val cal = Calendar.getInstance()
    val today = Calendar.getInstance()

    cal.time = date

    return when {
        cal.get(Calendar.YEAR) == today.get(Calendar.YEAR) &&
                cal.get(Calendar.DAY_OF_YEAR) == today.get(Calendar.DAY_OF_YEAR) -> {
            SimpleDateFormat("HH:mm", Locale.getDefault()).format(date)
        }
        cal.get(Calendar.YEAR) == today.get(Calendar.YEAR) &&
                cal.get(Calendar.DAY_OF_YEAR) == today.get(Calendar.DAY_OF_YEAR) - 1 -> {
            "Yesterday"
        }
        else -> {
            SimpleDateFormat("dd/MM/yy", Locale.getDefault()).format(date)
        }
    }
}
