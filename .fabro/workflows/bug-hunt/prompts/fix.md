Now prove and fix the bug you identified.

## Order of operations (regression-test-first)

1. **Write a test that reproduces the bug and FAILS on the current code.** Run
   it, and confirm it fails for the *right reason* (the defect), not a typo.
   This is the proof the bug is real — without a failing test, stop and
   reconsider whether it's actually a bug.
2. **Make the minimal fix.** Change as little production code as possible to
   correct the behavior. No refactors, no drive-by cleanups, no reformatting
   unrelated code. Preserve public API and existing behavior everywhere else.
3. **Confirm the test now passes** and that you haven't broken neighbors.

## Constraints

- Keep the diff surgical and easy to review: ideally one root-cause fix + its
  regression test.
- Honor `-D warnings`: no new clippy warnings, no `#[allow]` to dodge them,
  rustfmt-clean.
- If, while fixing, you discover the "bug" isn't actually wrong (the behavior
  is intended elsewhere), back out and clearly report that the hunt was a false
  alarm rather than forcing a change.

## Self-verify before finishing (required)

```
bash .fabro/verify.sh
```

Do not finish until it's green. The next node runs this exact gate as a hard
gate; a failure there produces no PR.
