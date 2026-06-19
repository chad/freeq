# Ambient research — topic recognition + text-channel assistance

## Goal (Chad)
When NOT addressed directly, Alexandria should:
1. Do high-level topic recognition of the conversation she hears.
2. Log topics in a note.
3. For topics that could use help (technical terms, concepts), search for relevant
   info / define terms.
4. Surface it via visualizations and/or links + messages in the **text channel**.

## Design
New module `freeq-eliza/src/research.rs` — a silent monitor, sibling to:
- `proactive.rs` (decides *when to speak*) and
- `ambient.rs` (decides *how the tile looks*).
Research decides *what helpful reference to drop in the text channel*.

`spawn_monitor(cfg, handle, channel, active) -> JoinHandle` (matches proactive).
Stored on `ActiveCall::research_task`, aborted on call-end via Drop.

### Loop (run_monitor)
- TICK 30s, skip first (settle).
- Snapshot transcript + speaker + video under the call lock; filter her own lines.
- Skip tick if < MIN_NEW_WORDS (25) of new transcript.
- One fast LLM call (`groq_chat_model`) — RECOGNIZE: list current topics + pick at
  most ONE help-worthy technical/factual topic (DEFAULT none; not chitchat/opinion;
  not in recently-covered list). Returns `{topics:[..], help:{topic,query}|null}`.
- LOG every recognized topic to the note file `~/.freeq/bots/<nick>/topics.jsonl`
  ({ts, channel, topics}).
- If `help` present AND not recently covered AND not mid-answer AND cooldown (75s)
  passed: RESEARCH via the web-search model (`voice_search_model` = groq/compound)
  → 1-2 sentence definition + up to 2 links. POST to the text channel via privmsg.
  Light visualization: set the tile ambient chip to the topic. Record topic +
  reset cooldown.

### Guardrails (anti-spam)
- 75s cooldown between posts; dedup against last ~24 topics; never while speaking;
  min new-words gate; off unless `--ambient-research`.

### Config
- `RunConfig.research_enabled` (+ `SharedConfig`), gated `&& !external_brain`.
- main.rs flag `--ambient-research` (default off).
- Enable for Alexandria in yokotabot `av.ts` eliza args.

## v1 scope
Topic recognition + note log + search + text-channel message with links + tile
concept chip. (Richer diagram/SVG visualizations = follow-up iteration.)

## Status
- (start) designed; implementing research.rs + wiring.
- DONE + verified live on staging. New `research.rs` monitor; RunConfig/SharedConfig
  `research_enabled`; `--ambient-research` flag; spawned in start_transcription +
  aborted on call-end; enabled for Alexandria in yokotabot av.ts.
  TEST (technical conversation, un-addressed): recognized topics
  ["Rough Consensus","Distributed Systems","Async Runtime","CAP Theorem"], logged to
  ~/.freeq/bots/alexandria/topics.jsonl, researched + posted a clean CAP-theorem
  definition card to #chadtest, set the tile chip. Web-search source link appended
  when the model actually searches. Anti-spam: 30s tick, ≥25 new words, 75s cooldown,
  dedup, never mid-answer.
- Follow-up ideas: richer diagram/SVG visualizations (currently chip + text+link);
  switch research_topic to streaming for more reliable link extraction.
