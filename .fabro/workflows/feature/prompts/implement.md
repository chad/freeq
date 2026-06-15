Implement the approved plan (in `/tmp/plan.md` and the context above).

## The feature

{{ goal }}

## Rules

- **Build only the in-scope work**: CI-buildable Rust crates (`freeq-server`, `freeq-sdk`, `freeq-sdk-ffi`, `freeq-tui`, `freeq-bots`, `freeq-windows-core`), the web app `freeq-app`, and protocol/docs. **Do NOT touch** `freeq-ios/`, `freeq-macos/`, `Freeq.WinUI/`, `freeq-windows-app/`, or the AV crates — they can't be compiled or verified here, so editing them would ship unverified code.
- **Follow the plan.** If you discover the plan is wrong mid-build, make the smallest correct deviation and note it clearly in your final message — don't silently rewrite the design.
- **Match the codebase**: existing patterns, error handling, naming. Honor `-D warnings` (no `#[allow]` to dodge lints; rustfmt + clippy clean). Keep protocol changes additive and versioned per the plan.
- **Write the tests the plan specified** — server/SDK unit + integration, web vitest — so each success criterion is pinned by a deterministic, offline test.
- Keep the diff coherent and reviewable. No drive-by reformatting of untouched code.

## Build discipline (important — the workspace is large)

- **Iterate with TARGETED commands** as you build: `cargo check -p <crate>`, `cargo test -p <crate> <name>`, `cargo clippy -p <crate>`, and scoped `npm run test` in `freeq-app`. Full-workspace builds are slow — don't loop them.
- **Do NOT touch build configuration** to speed things up — no `.cargo/config.toml`, no `[profile.*]` / `debuginfo` / `strip` edits, no `CARGO_*` changes. The environment is already tuned; let slow compiles run. Optimizing the build is not the task.

## Work incrementally, self-verify once at the end

Build in plan order, checking each crate with targeted commands as you go. When the slice is complete, first **auto-format and lint-fix** — the gate enforces both:

```
cargo fmt --all                       # gate runs `cargo fmt --all -- --check`
cargo clippy -p <crate> --tests       # fix every warning; never silence with #[allow]
```

(If you touched `freeq-app`, also run `npm run lint` / `npx prettier -w` there as the project does.) Then run the full gate **exactly once** to confirm it's green:

```
bash .fabro/verify.sh
```

If the feature is large, it's fine to land a coherent, gate-passing **first slice** that fully implements part of the plan rather than a broken attempt at all of it — but say exactly what you did and didn't implement in your final message, so the PR and any follow-up run are honest about remaining work.
