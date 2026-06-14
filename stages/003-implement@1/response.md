The work is done. Here's a summary of what was added and the state of verification:

---

## Summary

**File changed:** `freeq-sdk/src/client.rs` — 284 lines added, 0 changed, 0 deleted.

**9 new tests** added to the existing `irc_loop_tests` module, covering the 6 inbound IRC dispatch paths that had zero tests:

| Test | What it pins |
|------|-------------|
| `kick_emits_kicked_event` | `KICK` → `Event::Kicked` with correct channel/victim/kicker/reason |
| `kick_without_reason_has_empty_reason` | `KICK` with no reason param → `reason = ""` |
| `invite_emits_invited_event` | `INVITE` → `Event::Invited` with channel and sender nick |
| `away_with_message_emits_away_changed_some` | `AWAY :msg` → `AwayChanged { away_msg: Some(...) }` |
| `away_without_message_emits_away_changed_none` | `AWAY` (no params) → `AwayChanged { away_msg: None }` |
| `topic_change_emits_topic_changed_with_set_by` | Live `TOPIC` → `TopicChanged { set_by: Some(nick) }` |
| `rpl_topic_332_emits_topic_changed_without_set_by` | RPL_TOPIC 332 → `TopicChanged { set_by: None }` |
| `quit_emits_user_quit_event` | `QUIT :reason` → `Event::UserQuit { nick, reason }` |
| `nick_change_emits_nick_changed_event` | `NICK newnick` → `Event::NickChanged { old_nick, new_nick }` |

**Verification results:**
- `cargo fmt --check` ✅ passes
- `cargo check --workspace` (CI exclusions) ✅ passes  
- `cargo clippy --workspace` (CI exclusions) ✅ passes  
- `cargo test -p freeq-sdk` ✅ **149 tests, 0 failures** (22 in `irc_loop_tests`, including all 9 new ones)

The workspace-level `cargo test` failed during **linking** of `freeq-server` test binaries due to disk exhaustion (`No space left on device` / `Bus error` from the linker). This is a pre-existing infrastructure constraint — the 98GB disk was already at 95%+ when this session started, filled by a prior `cargo test --workspace` run. The failure is unrelated to the added test code.