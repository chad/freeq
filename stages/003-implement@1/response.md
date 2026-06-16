All 30 tests in `irc_loop_tests` pass (16 new + 14 existing). Let me do a final summary of what was added:

---

## Summary

**File modified:** `freeq-sdk/src/client.rs`

**16 new tests** added to the existing `mod irc_loop_tests` block, covering 7 previously untested IRC message types:

| Group | Tests | Behaviors pinned |
|---|---|---|
| **NICK change** | `server_nick_change_emits_nick_changed_event`, `server_nick_change_bare_prefix_parses_correctly` | nick parsed from prefix (with and without `!user@host`), new_nick from params |
| **QUIT** | `server_quit_emits_user_quit_event`, `server_quit_with_no_reason_emits_empty_reason` | quit with reason, quit with no params → empty reason (no panic) |
| **KICK** | `server_kick_emits_kicked_event`, `server_kick_with_no_reason_emits_empty_reason` | full kick with reason, kick with missing reason param |
| **AWAY** | `server_away_with_message_emits_away_changed`, `server_away_with_no_message_emits_away_changed_none` | going away (`Some(msg)`), returning (`None`) |
| **TOPIC** | `server_topic_change_emits_topic_changed_event`, `server_rpl_topic_332_emits_topic_changed_no_setter` | live TOPIC with `set_by`, 332 RPL_TOPIC with `set_by=None` |
| **INVITE** | `server_invite_emits_invited_event` | channel and inviter nick extracted correctly |
| **NAMES/TAGMSG** | `server_353_emits_names_event`, `server_tagmsg_emits_tag_msg_event` | nick list parsing, tag propagation to `TagMsg` event |
| **Legacy multiline** | `legacy_multiline_tag_normalizes_slash_n_to_newline`, `privmsg_without_multiline_tag_preserves_literal_slash_n` | `+freeq.at/multiline` tag triggers `\n` normalization; absence preserves verbatim body |
| **Extended JOIN** | `extended_join_with_did_account_emits_joined_with_account`, `extended_join_with_star_account_emits_joined_no_account` | DID account present, `*` sentinel → `None` |

All tests use the same in-process `tokio::io::duplex` harness as existing tests — no network, no server, deterministic, fast (0.13s for all 30).