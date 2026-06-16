Write a concrete, reviewable implementation plan for the feature, grounded in your exploration.

## The feature

harden and enhance file upload across the freeq server and web app, with on-freeq-server storage as the default

## Scope fence (critical)

**In scope — you may edit, and CI can verify:** the Rust crates that build in CI (`freeq-server`, `freeq-sdk`, `freeq-sdk-ffi`, `freeq-tui`, `freeq-bots`, `freeq-windows-core`), the web app `freeq-app`, and protocol/docs (`docs/PROTOCOL.md`, `CLAUDE.md`).

**Out of scope — do NOT plan to edit (this Linux executor cannot compile or verify them):** `freeq-ios/`, `freeq-macos/`, `Freeq.WinUI/`, `freeq-windows-app/` (Swift / WinUI), and the AV crates CI excludes (`freeq-av`, `freeq-eliza`, `freeq-av-client`, `freeq-av-image`).

The plan must keep the **protocol/server/web contract stable and additive** so the native clients can adopt it later without breaking — and must list exactly what each native client will need, as a dedicated follow-up section (not implemented here).

## Write the plan to a file AND output it

Write the full plan to `/tmp/plan.md` (it lives in the sandbox, not the repo, so it won't end up in the PR), and also print it as your final message so reviewers and the human gate can see it.

## Plan structure

1. **Goal & success criteria** — what "done" means, in testable terms.
2. **Design** — the approach: data model, API/protocol changes (additive, versioned), storage (the on-server-storage default and how it's selected), auth/capability, and how the web app consumes it.
3. **Work breakdown** — ordered, concrete steps with the specific files/functions each touches. Mark what's covered by the existing CI gate.
4. **Test strategy** — unit + integration tests you'll add (server, SDK, web), and how each success criterion is pinned.
5. **Migration / back-compat** — how existing uploads/data and older clients keep working.
6. **Follow-up: native clients** — exactly what `freeq-ios`, `freeq-macos`, and the Windows app must change to adopt this, as separate future PRs. Enumerated, not implemented.
7. **Risks & open questions.**

Keep it specific and honest. A plan reviewer should be able to find holes in it.