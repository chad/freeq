# Smart hand-off — self / web-search / Claude Code

## Goal (Chad)
She should figure out, per request, which path it needs:
- **handle herself** — conversational, definitions, opinions, general knowledge → fast voice model
- **web search** — current/real-world facts (weather, prices, scores, news) → search model
- **Claude Code** — agentic TASKS needing tools/files/multi-step work → offload to the
  claude -p brain over the seam

## Current state (the gap)
- Routing is binary: `external_brain` ON forwards EVERYTHING to Claude; OFF answers
  EVERYTHING natively. Alexandria runs native → never offloads.
- `qa::route_question` only classifies {visual, live_data}.
- The seam only connects when `external_brain`, and only sends `Utterance`.
- BUT the yokota side (av.ts) already handles a `delegate` seam msg → `onDelegate`:
  speaks "On it — handing that to Claude Code", runs the full brain, speaks the result.
  So the plumbing exists; eliza just never sends `delegate`.

## Changes
1. **qa.rs**: `QuestionRoute += agent: bool` (derive Copy). Router prompt gains the
   `agent` dimension (a TASK needing a coding/agentic assistant w/ tools+files: write/
   edit code, run/build/test/debug, create/modify files, multi-step jobs, deep
   research-and-synthesis, self-modification — NOT a talkable question, NOT a simple
   facts lookup). max_tokens 40→64.
2. **brain_seam.rs**: re-add `Outbound::Delegate { nick, text }` (serde "delegate").
3. **irc.rs**: connect the seam whenever `brain_sock` is set (native too), not only
   external_brain.
4. **irc.rs answer_and_speak**: after the route resolves, if `!external_brain &&
   route.agent && seam present` → set thinking, send `Delegate{asker,question}`, end the
   speak task, return (no native answer). Precedence: agent > visual > live_data > self.
5. **av.ts**: already handles `delegate`. No change.

## Test
Three probes (un-addressed phrasing avoided; address her): a chat question ("what's a
monad?" → self), a live-data question ("weather in NYC tomorrow" → search), and a task
("write a python script that reverses a string" → Claude Code delegate). Confirm the
log shows the right route + delegate forwards to yokota and a result comes back.

## Status
- DONE + verified live on PRODUCTION (irc.freeq.at). Router gained `agent`;
  seam connects in native mode; `Outbound::Delegate` re-added; dispatch forwards
  Delegate on (router.agent OR is_agentic_task heuristic) && !visual. av.ts
  onDelegate unchanged.
  - "what is a monad" → self (native claude-sonnet-4-6).
  - "weather in NYC tomorrow" → live_data → groq/compound search.
  - "write a python script… save to a file" → agent → Claude Code: spoke
    "On it — handing that to Claude Code", ran claude -p, reported the result.
  Note: the 8B router under-detects coding tasks, so `qa::is_agentic_task` cue
  heuristic (verb + technical object, or self-mod) is UNION'd in and does the
  heavy lifting; both router and heuristic covered by unit tests.
