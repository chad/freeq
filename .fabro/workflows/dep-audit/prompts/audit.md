Audit freeq's dependencies for security advisories and pick ONE actionable
finding to fix. This step is **read-only** — investigate and choose.

## Gather findings

- Rust: `cargo audit` (CI installs and runs it — `cargo install cargo-audit
  --locked` if it's missing on this machine). Note advisory IDs (RUSTSEC-…),
  affected crate + version, and the patched version range.
- Web app: `cd freeq-app && npm audit --omit=dev` (and without the flag to see
  dev-only too). Note the advisory, package, and fixed version.
- Cross-check against `Cargo.lock` / `freeq-app/package-lock.json` to see how
  the vulnerable package is pulled in (direct vs transitive) and what else
  depends on it.

## Choose

Pick the single best candidate to fix in one clean PR:
- Prefer advisories fixable by a **patch/minor bump** (low blast radius) over
  ones that require a major/breaking upgrade.
- Prefer a direct dependency, or a transitive one bumpable via the lockfile
  alone, over one that needs a breaking change to a direct dep.
- Higher severity and actually-reachable code beats theoretical/dev-only — but
  an easy clean fix for a moderate issue is better than a risky fix for a
  severe one.

If every current advisory requires a breaking upgrade or has no fix available,
DO NOT force one — report that clearly and stop; this run should open no PR.

## Report

- The advisory (ID, severity, package, current → fixed version).
- Direct or transitive, and the bump path you'll take (edit `Cargo.toml` /
  `package.json`, or update the lockfile only).
- Why this fix is low-risk, and anything you'll watch for in verification.
