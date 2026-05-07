package com.freeq.model

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Schedule for the auto-reconnect backoff that runs after a Disconnected
 * event. Pure JVM, no Android runtime needed.
 */
class ReconnectBackoffTest {

    @Test fun first_attempt_waits_two_seconds() {
        assertEquals(2L, ReconnectBackoff.delaySeconds(1))
    }

    @Test fun doubles_each_attempt() {
        assertEquals(2L, ReconnectBackoff.delaySeconds(1))
        assertEquals(4L, ReconnectBackoff.delaySeconds(2))
        assertEquals(8L, ReconnectBackoff.delaySeconds(3))
        assertEquals(16L, ReconnectBackoff.delaySeconds(4))
    }

    @Test fun caps_at_thirty_seconds() {
        // 1L shl 5 == 32; clamped to 30.
        assertEquals(30L, ReconnectBackoff.delaySeconds(5))
        assertEquals(30L, ReconnectBackoff.delaySeconds(6))
        assertEquals(30L, ReconnectBackoff.delaySeconds(20))
        assertEquals(30L, ReconnectBackoff.delaySeconds(Int.MAX_VALUE))
    }

    @Test fun zero_or_negative_attempts_returns_zero() {
        assertEquals(0L, ReconnectBackoff.delaySeconds(0))
        assertEquals(0L, ReconnectBackoff.delaySeconds(-1))
    }
}
