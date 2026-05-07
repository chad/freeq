package com.freeq.model

/**
 * Pure routing predicate for chat target names.
 *
 * Names with no `#`/`&` prefix are peer nicks (DM buffers). Names that
 * begin with `#` or `&` are IRC channels. Empty / whitespace names go
 * to a throwaway buffer so the caller never pollutes either container.
 *
 * Lives outside `AppState` so the routing rule can be unit-tested
 * without an Android runtime, and so `getOrCreateChannel` and
 * `getOrCreateDM` always agree on the classification.
 */
internal object BufferRouter {
    enum class Target { CHANNEL, DM, INVALID }

    fun classify(name: String): Target {
        val trimmed = name.trim()
        if (trimmed.isEmpty()) return Target.INVALID
        return if (trimmed.startsWith("#") || trimmed.startsWith("&"))
            Target.CHANNEL else Target.DM
    }
}
