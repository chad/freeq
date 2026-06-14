**All gates pass.** Here's what was added:

## Summary

**File modified:** `freeq-tui/src/main.rs`

**Changes:** 26 new tests added to the existing `#[cfg(test)] mod tests` block. Zero production code changed. One import fix (removed unused `Buffer` from the test import).

### New tests by function:

**`extract_url` (9 tests)**
- Returns plain `https://` and `http://` URLs
- Strips all 5 trailing punctuation chars (`.`, `,`, `)`, `]`, `;`)
- Returns the *first* URL when multiple are present
- Suppresses `cdn.bsky.app` URLs → `None`
- Suppresses `/xrpc/` URLs → `None`
- Enforces the `> 10` length threshold boundary (9, 10, 11 chars)
- Returns `None` for plain text and empty string

**`format_file_size` (4 tests)**
- Zero bytes → `"0B"`
- Sub-KB range: 1, 512, 1023 B
- KB boundary: 1024 B → `"1.0KB"`, 1536 → `"1.5KB"`, 1 MB−1 → `"1024.0KB"`
- MB boundary: 1 MB → `"1.0MB"`, 2 MB, 10 MB

**`try_nick_complete` (9 tests)**
- First-word completion appends `": "` suffix
- Mid-sentence completion appends `" "` suffix
- Case-insensitive fragment matching
- `@` op sigil stripped before insertion
- `+` voice sigil stripped before insertion
- No match → editor text and cursor unchanged
- Empty fragment (cursor after space) → no-op
- Text after cursor is preserved intact
- Non-existent buffer → graceful no-op