# @freeq/pi — backlog

Things asked for or noticed, not yet done. Newest at the top. Each entry says
what exists today so the gap is precise rather than "make it better".

## Richer working status — SHIPPED 2026-09-05

**Shipped:** presence `doing` is now a phrase + tool detail + elapsed clock
("answering chad in #freeq-dev · bash: npm test · 1m"), rendered inside the
60-char wire budget by src/status.ts. Sources of the phrase: the user's
first prompt (gisted, scrubbed), "answering <nick> in <venue>", the handoff
title, or the model's own `status` tool action. Elapsed refreshes on a slow
timer so a watcher can tell thinking from stuck; the AV tile shows the
phrase as a working card during calls. Steps end at agent_settled and on
every task-release path, so nothing stale advertises.

**Still open (transport idea, not scheduled):** IRCv3 `metadata` for this
rather than overloading the presence status string — AWAY-adjacent fields
render as "not here" in some clients, the opposite of a working agent.

## SDK double-emits numeric 674 as prose

`client.ts` `_` fallback treats every numeric in `400..700` as an error
notice, so 674 (actor classes) is emitted twice — once structured, once as a
`ServerNotice` the UI prints verbatim. Exclude numerics that already have a
structured handler; add a test that a vendor numeric never surfaces as text.

## `whois` answers for one session of a multi-session DID

`/api/v1/users/{nick}/whois` resolves one session via `nick_to_session` and
reports only its channels. A DID with several sessions (scripts, reconnects)
gets a per-session view that can say "no channels" while another session is
in the room and talking. Aggregate across `did_sessions`, the way the roster
does.

## A secondary bot-kit session rebinds the installation's nick

Any short-lived process connecting on an installation's `did:key` under a
different nick makes `bind_identity` release the registered nick and claim
the new one. Two throwaway scripts renamed the flagship installation today.
A non-primary session should attach as a sibling without touching the
registration — probably an explicit opt-in on `FreeqBot.create`.

## Delegated access needs a signed cert on the live installation

Shipped server-side; the live agent's cert is unsigned until `/freeq
authorize` is run once. Not a bug — the correct refusal — but the feature is
invisible until that happens.
