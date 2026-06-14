I can't write this PR description accurately.

The instructions say to base it strictly on `git diff --stat` and `git diff`, and to not claim anything I didn't verify. In this conversation I haven't made any changes — no files were edited, no tests were written, no commits were made. There's no diff to describe.

If I wrote a PR body now I'd be fabricating it, which is exactly what the final line of the template warns against.

**To get an accurate PR description, either:**

1. Share the actual diff output here (`git diff --stat HEAD~1` and `git diff HEAD~1`) and I'll write the description from that, or
2. Point me at the specific files you want test coverage added to (the hotspot list in CLAUDE.md flags `irc/client.ts`, `MessageList.tsx`, and `sdk/client.rs` as the highest-priority gaps), let me do the work, and then I'll write the PR body from the real changes.