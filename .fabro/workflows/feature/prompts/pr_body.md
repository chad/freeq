Write the pull-request description for the feature work. Fabro opens the PR from the run branch; this is its body. Base it strictly on what actually changed (`git diff --stat`, `git diff`) and what happened in this run.

**Feature:** {{ goal }}

GitHub-flavored markdown:

## Summary
What this PR delivers, in 2–4 sentences, tied to the approved plan's success criteria.

## What changed
Bulleted, grouped by area (server / SDK / protocol / web). Note new endpoints/tags, the on-server-storage default, and any additive/versioned protocol changes.

## Scope & what's deferred
- State clearly that this covers the **Linux-verifiable scope** (Rust + web) only.
- List the **native-client follow-ups** (iOS / macOS / Windows) from the plan that are intentionally NOT in this PR.
- If the implementer landed a first slice rather than the whole plan, say exactly what's done vs remaining.

## Testing
The tests added and what they pin. Confirm `.fabro/verify.sh` (CI-mirrored: rustfmt + check + clippy `-D warnings` + test, plus web vitest) passes.

## Review notes
Summarize the peer review the change went through (security + correctness), and any findings consciously deferred.

Factual and precise. Don't claim anything the run didn't actually verify.
