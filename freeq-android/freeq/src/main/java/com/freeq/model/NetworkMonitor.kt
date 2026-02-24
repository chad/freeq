package com.freeq.model

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import androidx.compose.runtime.mutableStateOf
import kotlinx.coroutines.*

class NetworkMonitor(context: Context) {
    val isConnected = mutableStateOf(true)

    private val connectivityManager =
        context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    private var wasDisconnected = false
    private var appState: AppState? = null
    private val scope = CoroutineScope(Dispatchers.Main + SupervisorJob())

    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            scope.launch {
                isConnected.value = true
                if (wasDisconnected) {
                    wasDisconnected = false
                    attemptReconnect()
                }
            }
        }

        override fun onLost(network: Network) {
            scope.launch {
                isConnected.value = false
                wasDisconnected = true
            }
        }
    }

    init {
        // Check initial state
        val active = connectivityManager.activeNetwork
        val caps = active?.let { connectivityManager.getNetworkCapabilities(it) }
        isConnected.value = caps?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true

        val request = NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        connectivityManager.registerNetworkCallback(request, callback)
    }

    fun bind(appState: AppState) {
        this.appState = appState
    }

    private fun attemptReconnect() {
        val state = appState ?: return
        if (state.connectionState.value != ConnectionState.Disconnected) return
        if (state.nick.value.isEmpty()) return

        scope.launch {
            delay(1000)
            if (state.connectionState.value == ConnectionState.Disconnected) {
                state.connect(state.nick.value)
            }
        }
    }

    fun destroy() {
        connectivityManager.unregisterNetworkCallback(callback)
        scope.cancel()
    }
}
