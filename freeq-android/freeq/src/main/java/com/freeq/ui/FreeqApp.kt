package com.freeq.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import com.freeq.model.AppState
import com.freeq.model.ConnectionState
import com.freeq.ui.components.MotdDialog
import com.freeq.ui.navigation.MainScreen
import com.freeq.ui.screens.ConnectScreen
import com.freeq.ui.theme.FreeqTheme

@Composable
fun FreeqApp(appState: AppState) {
    val isDark by appState.isDarkTheme
    val connectionState by appState.connectionState

    // Auto-reconnect saved session on app start
    LaunchedEffect(Unit) {
        if (connectionState == ConnectionState.Disconnected && appState.hasSavedSession) {
            appState.reconnectSavedSession()
        }
    }

    FreeqTheme(darkTheme = isDark) {
        when (connectionState) {
            ConnectionState.Disconnected,
            ConnectionState.Connecting -> ConnectScreen(appState)

            ConnectionState.Connected,
            ConnectionState.Registered -> MainScreen(appState)
        }

        MotdDialog(appState)
    }
}
