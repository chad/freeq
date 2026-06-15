Apply the smallest change that resolves the advisory you chose.

## Rules

- **Minimal bump.** Prefer updating the lockfile to a patched version within
  the existing semver range (`cargo update -p <crate> --precise <ver>`, or
  `npm update <pkg>` / an exact lockfile edit). Only edit `Cargo.toml` /
  `package.json` version requirements if the lockfile-only path can't reach the
  fixed version.
- **No collateral churn.** Don't bump unrelated dependencies, don't run a
  blanket `cargo update` / `npm update` that rewrites the whole lockfile, and
  don't reformat manifests. Touch only what the fix requires.
- **Do not force breaking upgrades.** If the patched version pulls a major bump
  that breaks the build/API, stop and report it rather than papering over
  compile errors with code changes — that turns a dependency PR into a risky
  refactor. A clean "this needs a breaking upgrade, here's what it'd take" is a
  fine outcome (it will open no PR; that's correct).
- Re-run the relevant audit (`cargo audit` / `npm audit`) and confirm the
  advisory is actually gone.

## Build discipline (important — the workspace is large)

- While checking the bump, prefer targeted commands (`cargo check -p <crate>`,
  `cargo test -p <crate>`) over repeated full-workspace builds.
- **Do NOT touch build configuration** to speed things up — no
  `.cargo/config.toml`, no `[profile.*]`/`debuginfo`/`strip` edits, no `CARGO_*`
  changes. The environment is already tuned; let slow compiles run.

## Self-verify once, at the end (required)

After the bump, if you changed any Rust source, first `cargo fmt --all` and fix
any clippy warnings (the gate enforces both). Then run the full gate **exactly
once** to prove nothing broke:

```
bash .fabro/verify.sh
```

Don't loop the full gate. The next node runs it authoritatively; a failure
opens no PR.
