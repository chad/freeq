package com.freeq.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Reply
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import android.net.Uri
import com.freeq.model.AppState
import com.freeq.ui.theme.FreeqColors
import com.freeq.ui.theme.Theme

@Composable
fun ComposeBar(
    appState: AppState,
    modifier: Modifier = Modifier
) {
    var text by remember { mutableStateOf("") }
    var completions by remember { mutableStateOf<List<String>>(emptyList()) }
    var photoUri by remember { mutableStateOf<Uri?>(null) }
    val haptic = LocalHapticFeedback.current

    val replyingTo by appState.replyingTo
    val editingMessage by appState.editingMessage
    val activeChannel = appState.activeChannel.value

    // Pre-fill text when entering edit mode
    LaunchedEffect(editingMessage) {
        editingMessage?.let { text = it.text }
    }

    val canSend = text.isNotBlank()

    Column(modifier = modifier) {
        // Top border
        HorizontalDivider(color = MaterialTheme.colorScheme.outline.copy(alpha = 0.3f))

        // Nick autocomplete suggestions
        if (completions.isNotEmpty()) {
            LazyRow(
                modifier = Modifier
                    .fillMaxWidth()
                    .background(MaterialTheme.colorScheme.surface)
                    .padding(horizontal = 12.dp, vertical = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp)
            ) {
                items(completions) { nick ->
                    Surface(
                        shape = RoundedCornerShape(16.dp),
                        color = MaterialTheme.colorScheme.surfaceVariant,
                        modifier = Modifier.clickable { applyCompletion(nick, text) { text = it; completions = emptyList() } }
                    ) {
                        Row(
                            modifier = Modifier.padding(horizontal = 10.dp, vertical = 6.dp),
                            horizontalArrangement = Arrangement.spacedBy(4.dp),
                            verticalAlignment = Alignment.CenterVertically
                        ) {
                            UserAvatar(nick = nick, size = 20.dp)
                            Text(
                                nick,
                                fontSize = 13.sp,
                                fontWeight = FontWeight.Medium,
                                color = MaterialTheme.colorScheme.onBackground
                            )
                        }
                    }
                }
            }
        }

        // Reply context bar
        if (replyingTo != null) {
            ContextBar(
                icon = Icons.AutoMirrored.Filled.Reply,
                label = "Replying to ${replyingTo!!.from}",
                preview = replyingTo!!.text,
                color = FreeqColors.accent,
                onDismiss = { appState.replyingTo.value = null }
            )
        }

        // Edit context bar
        if (editingMessage != null) {
            ContextBar(
                icon = Icons.Default.Edit,
                label = "Editing message",
                preview = editingMessage!!.text,
                color = FreeqColors.warning,
                onDismiss = {
                    appState.editingMessage.value = null
                    text = ""
                }
            )
        }

        // Input area
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .background(MaterialTheme.colorScheme.surface)
                .padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.Bottom,
            horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            // Text field
            OutlinedTextField(
                value = text,
                onValueChange = { newText ->
                    text = newText
                    completions = updateCompletions(newText, appState)
                    if (newText.isNotEmpty()) {
                        activeChannel?.let { appState.sendTyping(it) }
                    }
                },
                modifier = Modifier.weight(1f),
                placeholder = {
                    val placeholder = when {
                        replyingTo != null -> "Reply..."
                        editingMessage != null -> "Edit message..."
                        else -> "Message ${activeChannel ?: ""}"
                    }
                    Text(placeholder, fontSize = 15.sp)
                },
                leadingIcon = {
                    // Photo picker inside text field
                    PhotoPickerButton(
                        appState = appState,
                        onPhotoPicked = { uri -> photoUri = uri }
                    )
                },
                maxLines = 6,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                keyboardActions = KeyboardActions(
                    onSend = {
                        if (canSend) {
                            haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                            send(text.trim(), appState) { text = ""; completions = emptyList() }
                        }
                    }
                ),
                shape = RoundedCornerShape(22.dp),
                colors = OutlinedTextFieldDefaults.colors(
                    focusedBorderColor = MaterialTheme.colorScheme.outline.copy(alpha = 0.5f),
                    unfocusedBorderColor = MaterialTheme.colorScheme.outline.copy(alpha = 0.3f),
                    focusedContainerColor = MaterialTheme.colorScheme.surfaceVariant,
                    unfocusedContainerColor = MaterialTheme.colorScheme.surfaceVariant,
                ),
                textStyle = LocalTextStyle.current.copy(fontSize = 15.sp)
            )

            // Send button
            IconButton(
                onClick = {
                    if (canSend) {
                        haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                        send(text.trim(), appState) { text = ""; completions = emptyList() }
                    }
                },
                enabled = canSend,
                modifier = Modifier
                    .size(40.dp)
                    .clip(CircleShape)
                    .background(
                        if (canSend) FreeqColors.accent
                        else MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.2f)
                    )
            ) {
                Icon(
                    imageVector = if (editingMessage != null) Icons.Default.Check else Icons.Default.ArrowUpward,
                    contentDescription = "Send",
                    tint = MaterialTheme.colorScheme.onPrimary,
                    modifier = Modifier.size(20.dp)
                )
            }
        }
    }

    // Photo preview sheet
    photoUri?.let { uri ->
        ImagePreviewSheet(
            uri = uri,
            appState = appState,
            onDismiss = { photoUri = null },
            onSent = { photoUri = null }
        )
    }
}

@Composable
private fun ContextBar(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    label: String,
    preview: String,
    color: androidx.compose.ui.graphics.Color,
    onDismiss: () -> Unit
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surface)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp)
    ) {
        Box(
            modifier = Modifier
                .width(3.dp)
                .height(32.dp)
                .background(color, shape = RoundedCornerShape(2.dp))
        )

        Icon(
            icon,
            contentDescription = null,
            modifier = Modifier.size(14.dp),
            tint = color
        )

        Column(modifier = Modifier.weight(1f)) {
            Text(
                text = label,
                fontSize = 12.sp,
                fontWeight = FontWeight.SemiBold,
                color = color
            )
            Text(
                text = preview,
                fontSize = 12.sp,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis
            )
        }

        IconButton(onClick = onDismiss, modifier = Modifier.size(24.dp)) {
            Icon(
                Icons.Default.Close,
                contentDescription = "Dismiss",
                modifier = Modifier.size(18.dp),
                tint = MaterialTheme.colorScheme.onSurfaceVariant
            )
        }
    }
}

private fun updateCompletions(text: String, appState: AppState): List<String> {
    val lastWord = text.split(" ").lastOrNull() ?: return emptyList()
    if (!lastWord.startsWith("@") || lastWord.length <= 1) return emptyList()

    val prefix = lastWord.drop(1).lowercase()
    val members = appState.activeChannelState?.members ?: return emptyList()
    return members
        .map { it.nick }
        .filter { it.lowercase().startsWith(prefix) && !it.equals(appState.nick.value, ignoreCase = true) }
        .sorted()
        .take(5)
}

private fun applyCompletion(nick: String, currentText: String, setText: (String) -> Unit) {
    val words = currentText.split(" ").toMutableList()
    if (words.isNotEmpty() && words.last().startsWith("@")) {
        words[words.lastIndex] = "@$nick"
    }
    setText(words.joinToString(" ") + " ")
}

private fun send(text: String, appState: AppState, onSent: () -> Unit) {
    val target = appState.activeChannel.value ?: return
    if (text.isEmpty()) return

    if (text.startsWith("/")) {
        handleCommand(text, appState)
    } else {
        appState.sendMessage(target, text)
    }
    onSent()
}

private fun handleCommand(input: String, appState: AppState) {
    val parts = input.drop(1).split(" ", limit = 2)
    val cmd = parts.firstOrNull()?.lowercase() ?: return
    val arg = parts.getOrNull(1)

    when (cmd) {
        "join" -> arg?.let { appState.joinChannel(it) }
        "part", "leave" -> appState.activeChannel.value?.let { appState.partChannel(it) }
        "nick" -> arg?.let { appState.sendRaw("NICK $it") }
        "me" -> {
            val target = appState.activeChannel.value ?: return
            arg?.let { appState.sendRaw("PRIVMSG $target :\u0001ACTION $it\u0001") }
        }
        "msg" -> {
            val msgParts = (arg ?: "").split(" ", limit = 2)
            if (msgParts.size == 2) {
                appState.sendMessage(msgParts[0], msgParts[1])
            }
        }
        "topic" -> {
            val target = appState.activeChannel.value ?: return
            arg?.let { appState.sendRaw("TOPIC $target :$it") }
        }
        else -> appState.sendRaw(input.drop(1))
    }
}
