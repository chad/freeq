package com.freeq.ui.screens

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.foundation.layout.*
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Group
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.freeq.model.AppState
import com.freeq.ui.components.ComposeBar
import com.freeq.ui.components.MemberList
import com.freeq.ui.components.MessageList

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChatDetailScreen(
    appState: AppState,
    channelName: String,
    onBack: () -> Unit,
    onNavigateToChat: ((String) -> Unit)? = null
) {
    val channelState = remember(channelName) {
        appState.channels.firstOrNull { it.name.equals(channelName, ignoreCase = true) }
            ?: appState.dmBuffers.firstOrNull { it.name.equals(channelName, ignoreCase = true) }
    }

    var showMembers by remember { mutableStateOf(false) }
    val isChannel = channelName.startsWith("#")

    // Update active channel and mark read
    LaunchedEffect(channelName) {
        appState.activeChannel.value = channelName
        appState.markRead(channelName)
    }

    // Mark read as messages arrive
    LaunchedEffect(channelState?.messages?.size) {
        appState.markRead(channelName)
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(
                            channelName,
                            fontSize = 17.sp,
                            fontWeight = FontWeight.SemiBold
                        )
                        if (isChannel && channelState != null) {
                            Text(
                                "${channelState.members.size} members",
                                fontSize = 12.sp,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                fontWeight = FontWeight.Normal
                            )
                        }
                    }
                },
                navigationIcon = {
                    IconButton(onClick = {
                        appState.activeChannel.value = null
                        onBack()
                    }) {
                        Icon(
                            Icons.AutoMirrored.Filled.ArrowBack,
                            contentDescription = "Back"
                        )
                    }
                },
                actions = {
                    if (isChannel) {
                        IconButton(onClick = { showMembers = !showMembers }) {
                            Icon(
                                Icons.Default.Group,
                                contentDescription = "Members",
                                tint = if (showMembers) MaterialTheme.colorScheme.primary
                                else MaterialTheme.colorScheme.onSurfaceVariant
                            )
                        }
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.surface,
                    titleContentColor = MaterialTheme.colorScheme.onSurface
                )
            )
        }
    ) { padding ->
        if (channelState == null) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(padding),
                contentAlignment = Alignment.Center
            ) {
                Text(
                    "Channel not found",
                    color = MaterialTheme.colorScheme.onSurfaceVariant
                )
            }
            return@Scaffold
        }

        Row(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
        ) {
            // Main content: messages + compose
            Column(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxHeight()
            ) {
                // Topic bar
                val topic by channelState.topic
                if (topic.isNotEmpty()) {
                    Surface(
                        color = MaterialTheme.colorScheme.surfaceVariant,
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text(
                            text = topic,
                            modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                            fontSize = 13.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 2
                        )
                    }
                }

                // Messages
                MessageList(
                    appState = appState,
                    channelState = channelState,
                    modifier = Modifier.weight(1f)
                )

                // Compose bar
                ComposeBar(appState = appState)
            }

            // Member list (side panel)
            AnimatedVisibility(
                visible = showMembers,
                enter = slideInHorizontally { it },
                exit = slideOutHorizontally { it }
            ) {
                Surface(
                    modifier = Modifier.width(240.dp),
                    color = MaterialTheme.colorScheme.surface,
                    shadowElevation = 4.dp
                ) {
                    MemberList(
                        members = channelState.members,
                        onMemberClick = { nick ->
                            if (!nick.equals(appState.nick.value, ignoreCase = true)) {
                                appState.getOrCreateDM(nick)
                                showMembers = false
                                onNavigateToChat?.invoke(nick)
                            }
                        }
                    )
                }
            }
        }
    }
}
