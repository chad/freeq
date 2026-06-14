Now add the tests for the file you chose. Goal: a tight, high-signal coverage
increase that lands as a clean, mergeable PR.

## Rules

- **Tests only.** Do not change production behavior. The only production edits
  allowed are *minimal, behavior-preserving* testability seams (e.g. making a
  pure helper `pub(crate)`, extracting an existing expression into a named
  function) — and only if genuinely necessary. If a target can't be tested
  without real behavior change, narrow your scope to what can.
- **Follow existing conventions.** Match the sibling test file's structure,
  naming, imports, and assertion style. Rust: a `#[cfg(test)] mod tests` block
  or a `tests/` integration file as the crate already does. TS/React:
  `*.test.ts(x)` with the same test runner (vitest) and helpers already in use.
- **Pin real behavior, including edge cases**: error paths, boundary inputs,
  empty/oversized/malformed inputs, and the specific behaviors you listed.
  Prefer deterministic, offline tests — no live network, no sleeps, no clock
  flakiness.
- **Do not weaken `-D warnings`.** No `#[allow(...)]` to paper over lints; no
  `console.log`/dead code. New test code must be clippy-clean and rustfmt-clean.

## Self-verify before finishing (required)

Run the same gate CI will run, and do not finish until it passes:

```
bash .fabro/verify.sh
```

If it fails, read the output and fix your tests (or back out an over-broad
change) until it's green. The next node runs this exact gate as a hard gate —
if it fails there, the run produces no PR and the work is wasted, so make it
pass here first.

Keep the diff focused: one file's worth of new tests (plus its minimal seam if
any). Don't reformat unrelated code or bundle drive-by changes.
