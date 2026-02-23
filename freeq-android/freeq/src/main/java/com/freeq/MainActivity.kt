package com.freeq

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.lifecycle.viewmodel.compose.viewModel
import com.freeq.model.AppState
import com.freeq.ui.FreeqApp

class MainActivity : ComponentActivity() {
    private var appState: AppState? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()

        setContent {
            val state: AppState = viewModel()
            appState = state

            // Handle initial deep link if launched via OAuth callback
            intent?.data?.let { handleOAuthCallback(it, state) }

            FreeqApp(appState = state)
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        // Handle OAuth callback when activity already running (singleTask)
        intent.data?.let { uri ->
            appState?.let { handleOAuthCallback(uri, it) }
        }
    }

    private fun handleOAuthCallback(uri: Uri, state: AppState) {
        if (uri.scheme != "freeq") return

        val token = uri.getQueryParameter("token") ?: return
        val nick = uri.getQueryParameter("nick") ?: return
        val did = uri.getQueryParameter("did")

        state.pendingWebToken = token
        did?.let { state.authenticatedDID.value = it }
        state.serverAddress.value = "irc.freeq.at:6667"
        state.connect(nick)
    }
}
