You are the lead engineer again. Three independent reviewers — security, compatibility, and storage/ops — have critiqued your plan (their critiques are in the conversation context above).

## The feature

harden and enhance file upload across the freeq server and web app, with on-freeq-server storage as the default

## What to do

1. Work through every reviewer item. For each: either fold the fix into the plan, or explicitly justify why you're not (with reasoning a reviewer would accept). Don't silently drop anything.
2. Resolve conflicts between reviewers with an explicit decision.
3. Re-confirm the scope fence: in-scope = CI-buildable Rust crates + `freeq-app` + protocol/docs; native clients (iOS/macOS/WinUI) are follow-ups only.

## Output

Rewrite the full revised plan to `/tmp/plan.md` (overwrite it), and print it as your final message. At the top, add a short "## Changes from review" section listing what you changed and what you consciously rejected and why. This revised plan is what the human will approve and what the implementer will build — make it the single source of truth.