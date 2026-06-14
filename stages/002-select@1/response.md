Now I have a complete picture. Let me summarize my findings.

---

## My Choice: `freeq-tui/src/main.rs`

**File:** `freeq-tui/src/main.rs` — Rust, `freeq-tui` crate

### Why it's high-risk and undertested

The hotspot analysis gives it a **gamma score of 9** (2,874 lines, 28 tracked functions, churn ×3) — putting it firmly in the top 15 risky files. More critically:

- The `#[cfg(test)] mod tests` block at line 2779 covers **only one thing**: the `parse_join_prefix` function (9 test cases). That's 9 tests pinning a ~30-line helper.
- Every other non-trivial free function in the file — `extract_url`, `format_file_size`, `format_link_preview`, `try_nick_complete`, `format_timestamp`/`parse_timestamp_ms` — has **zero dedicated tests**.
- These functions contain real logic and real edge-case behavior (stripping punctuation from URLs, skipping freeq-internal URLs, byte-level size formatting, word-boundary nick completion, RFC 3339 timestamp parsing/fallback).
- `freeq-tui/src/app.rs` (gamma 7) does have 42 tests but they cover `app.rs`'s structs, not `main.rs`'s helpers.

### 3–6 specific behaviors I intend to pin

1. **`extract_url` — trailing punctuation stripping.** A URL followed by `)`, `.`, `,` or `]` must return the URL without the trailing char. A "URL" shorter than 10 chars must not be returned.
2. **`extract_url` — internal URL suppression.** URLs containing `cdn.bsky.app` or `/xrpc/` must return `None` even when they are otherwise valid.
3. **`format_file_size` — boundary conditions.** 0 B → `"0B"`, 1023 B → `"1023B"`, 1024 B → `"1.0KB"`, 1024·1024 B → `"1.0MB"`, fractional MB case.
4. **`try_nick_complete` — first-word completion adds `: ` suffix; mid-sentence completion adds space.** Input `"ali"` at cursor 3 with `["alice", "bob"]` in nicks → `"alice: "`.
5. **`try_nick_complete` — no match leaves editor unchanged.** Fragment not matching any nick must not corrupt `editor.text` or `editor.cursor`.
6. **`extract_url` — multiple words; returns only the first valid URL.** Input `"see https://example.com and https://other.org"` → `"https://example.com"`.

### Existing test convention to follow

The sibling test module lives at the bottom of `freeq-tui/src/main.rs` (line 2779):

```rust
#[cfg(test)]
mod tests {
    use super::parse_join_prefix;
    …
}
```

New tests go in that same `mod tests` block. The pattern is: `use super::<fn>`, then plain `#[test]` functions with `assert_eq!` / `assert!`. No async, no network — purely functional unit tests. That matches `freeq-tui/src/app.rs`'s test style too.