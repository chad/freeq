---
name: freeqcc-stop
description: >
  Stop the running freeqcc daemon. Sends SIGTERM; the daemon does a
  clean QUIT (PRESENCE :state=offline + IRC QUIT) before exiting. Use
  when the user wants to take their agent offline.
---

# freeqcc stop

Run `freeqcc stop` and report:

- "Sent SIGTERM to pid <N>." → the daemon is shutting down cleanly
- "No daemon is running" → nothing was up
- "Pid <N> is gone; cleaning up stale pid file" → leftover pid file
  from a previous unclean shutdown; CLI cleaned it up automatically

Optionally re-run `freeqcc status` after a beat to confirm
`daemon: not running`. No need to babysit this one — SIGTERM is
typically processed within a second.

To restart, the user runs `/freeqcc-launch` (or `freeqcc launch
--detach` directly).
