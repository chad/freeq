package com.freeq.model

/**
 * Pure schedule for the auto-reconnect backoff that fires off the
 * Disconnected event handler. `2^attempts` seconds, clamped at 30.
 */
internal object ReconnectBackoff {
    private const val MAX_SECONDS: Long = 30L
    private const val MAX_SHIFT: Int = 5  // 1L shl 5 == 32 → already over the clamp

    fun delaySeconds(attempts: Int): Long {
        if (attempts <= 0) return 0L
        return minOf(1L shl minOf(attempts, MAX_SHIFT), MAX_SECONDS)
    }
}
