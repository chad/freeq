package com.freeq.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.freeq.model.AppState
import com.freeq.model.ConnectionState
import com.freeq.ui.components.MotdDialog
import com.freeq.ui.navigation.MainScreen
import com.freeq.ui.screens.ConnectScreen
import com.freeq.ui.theme.FreeqColors
import com.freeq.ui.theme.FreeqTheme
import kotlinx.coroutines.delay

@Composable
fun FreeqApp(appState: AppState) {
    val isDark by appState.isDarkTheme
    val connectionState by appState.connectionState
    val loggedOut by appState.loggedOut

    // Track whether user cancelled auto-reconnect
    var userCancelledReconnect by remember { mutableStateOf(false) }

    // Auto-reconnect saved session on app start
    LaunchedEffect(Unit) {
        if (connectionState == ConnectionState.Disconnected && appState.hasSavedSession) {
            appState.reconnectSavedSession()
        }
    }

    // Reset cancel flag on successful connection
    LaunchedEffect(connectionState) {
        if (connectionState == ConnectionState.Connected || connectionState == ConnectionState.Registered) {
            userCancelledReconnect = false
        }
    }

    FreeqTheme(darkTheme = isDark) {
        when {
            // Connected — show main screen
            connectionState == ConnectionState.Connected ||
            connectionState == ConnectionState.Registered -> MainScreen(appState)

            // Auto-reconnecting with saved session — show spinner, not login form
            appState.hasSavedSession && !userCancelledReconnect && !loggedOut -> ReconnectingScreen(
                onCancel = {
                    userCancelledReconnect = true
                    appState.disconnect()
                }
            )

            // No saved session or user cancelled — show login form
            else -> ConnectScreen(appState)
        }

        MotdDialog(appState)
    }
}

@Composable
private fun ReconnectingScreen(onCancel: () -> Unit) {
    var elapsedSeconds by remember { mutableIntStateOf(0) }

    LaunchedEffect(Unit) {
        while (true) {
            delay(1000)
            elapsedSeconds++
        }
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(FreeqColors.bgPrimaryDark),
        contentAlignment = Alignment.Center
    ) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(16.dp)
        ) {
            CircularProgressIndicator(
                color = FreeqColors.accent,
                modifier = Modifier.size(32.dp),
                strokeWidth = 3.dp
            )
            Text(
                text = if (elapsedSeconds < 8) "Connecting..." else "Still connecting...",
                fontSize = 15.sp,
                color = FreeqColors.textMutedDark
            )
            if (elapsedSeconds >= 15) {
                TextButton(onClick = onCancel) {
                    Text(
                        "Sign in manually",
                        fontSize = 14.sp,
                        color = FreeqColors.accent
                    )
                }
            }
        }
    }
}
