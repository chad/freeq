Two independent reviewers — correctness and security — have reviewed your diff (findings are in the context above).

## The feature

{{ goal }}

## What to do

1. **Fix every must-fix finding.** For each, make the change and note which finding it resolves.
2. For nice-to-haves: fix the cheap, clearly-correct ones; for the rest, leave a brief note (in your final message) on why you're deferring — don't silently ignore.
3. If a finding is wrong (the reviewer misread the code), say so explicitly with the evidence rather than making a needless change.
4. Stay inside the scope fence (no native-client / AV-crate edits) and keep the diff coherent — honor `-D warnings`, rustfmt + clippy clean.

## Self-verify

Run the gate and don't finish until it's green:

```
bash .fabro/verify.sh
```

The next node runs this as a hard gate. In your final message, summarize what you fixed, what you deferred and why, and any finding you rejected.
