Write the pull-request description for the dependency fix. Base it strictly on
what you changed (`git diff`). GitHub-flavored markdown:

## Advisory
The ID (RUSTSEC-… / GHSA-… / npm advisory), severity, affected package, and a
one-line description of the vulnerability.

## Change
Package `current → fixed` version, whether it was a direct or transitive
dependency, and exactly what you edited (lockfile only, or a manifest version
bump).

## Why it's safe
Why this is the smallest viable change and not a breaking upgrade; note that no
unrelated dependencies were churned.

## Verification
- The audit (`cargo audit` / `npm audit`) no longer reports the advisory.
- `.fabro/verify.sh` (CI-mirrored: rustfmt + check + clippy -D warnings + test)
  passes.

Factual and concise.
