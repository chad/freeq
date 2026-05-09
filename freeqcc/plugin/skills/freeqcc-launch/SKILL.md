---
name: freeqcc-launch
description: >
  Launch a freeqcc agent — a freeq-DM-controllable Claude Code session
  with a cryptographic did:key identity, owner-DID-gated access, and a
  declarative provenance cert. Use when the user wants to start their
  agent and become reachable via Bluesky DM.
---

# freeqcc launch

The `freeqcc` CLI runs the agent. Your job here:

1. Run `freeqcc launch --detach` (background mode).
   - First-time users will be prompted in the **terminal where you
     run the command** for their AT Protocol handle and a bot nick.
     Make sure they know to look there if the command appears to
     hang — the prompts library doesn't echo to Claude Code's
     output stream.
   - On subsequent launches the daemon starts immediately because
     handle + nick are already persisted in `~/.freeqcc/`.

2. After launch returns, run `freeqcc status` to read back the live
   state and report:
   - the bot's IRC nick + did:key DID
   - the owner DID we'll only respond to
   - provenance verified status (expected: `verified=false`,
     reason "Cert has no signature; declarative only" — that's
     intentional in v1.0)

3. Tell the user how to DM it: from any freeq-connected client
   signed in as the owner handle, send a Bluesky DM to the bot's
   nick. The agent will dispatch to a persistent `claude -p`
   session and reply.

If `freeqcc` isn't on PATH, ask the user to install:

    cd /path/to/freeq/freeqcc && npm install && npm link

If the user wants to stop the agent later, the matching skills are
`/freeqcc-status` and `/freeqcc-stop`.
