---
name: freeqcc-status
description: >
  Show the live state of the user's freeqcc daemon: connected? owner
  verified? bot DID/nick? provenance status? Use when the user asks
  "is my agent up?" or wants a quick health check.
---

# freeqcc status

Run `freeqcc status` and pretty-print the result. The CLI prints:

- daemon liveness (pid file + `kill -0` check)
- bot nick from `~/.freeqcc/config.json`
- owner handle/DID from `~/.freeqcc/owner.json`
- agent did:key from `~/.freeqcc/agent.key`
- delegation cert state (signed vs unsigned v1.0)
- server URL
- if running: `actor.online`, `actor.nick`, and provenance
  `_verified` + `_verification_reason` from the live `/api/v1/actors/{did}`

If the daemon is not running, tell the user to run `/freeqcc-launch`.

If `freeqcc doctor` would help (e.g. config seems corrupt), surface
that as a follow-up.
