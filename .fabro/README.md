# Fabro — agents improving freeq around the clock

This directory configures [Fabro](https://fabro.sh) to run AI-agent workflows
that continuously improve freeq, each opening a **ready-for-review PR** you
merge by hand. Workflows are version-controlled DOT graphs — the process is
code, reviewable and forkable like anything else in the repo.

## The workflows

| Workflow | What it does | Cadence (when enabled) |
|---|---|---|
| [`test-coverage`](workflows/test-coverage/) | Picks a high-risk, undertested file (via `scripts/hotspots.sh` + the CLAUDE.md hotspot list) and adds focused tests. | Nightly 02:00 UTC |
| [`bug-hunt`](workflows/bug-hunt/) | Finds one genuine correctness bug, pins it with a failing regression test, fixes it minimally. Uses **opus** for the hunt. | Nightly 04:00 UTC |
| [`dep-audit`](workflows/dep-audit/) | Runs `cargo audit` + `npm audit`, patches one advisory with the smallest viable bump. | Weekly Mon 06:00 UTC |

Each is the same shape: **select/hunt/audit (read-only) → implement → CI gate →
PR body**. The CI gate ([`verify.sh`](verify.sh)) mirrors `.github/workflows/ci.yml`
exactly (rustfmt + `cargo check`/`clippy -D warnings`/`test` with CI's AV-crate
exclusions, plus `freeq-app` vitest when the app changed). It's a `goal_gate`:
**a failed gate opens no PR**, and routes back to the implementer for up to 2
bounded retries first. So a PR only ever appears when the work is genuinely green.

## PR policy

Set in [`project.toml`](project.toml): PRs are opened **ready for review** (not
draft), squash-merge, **never auto-merged** — a human clicks merge. Default
model is `claude-sonnet-4-6`; the bug-hunt finder overrides to `claude-opus-4-8`.

## Where it runs

An always-on **boxd VM** (`fabro-freeq`, auto-suspend disabled so cron fires)
hosts the Fabro server under systemd. The VM is the isolation boundary from your
laptop — the laptop need not stay awake.

Runs execute via the **docker provider** (boxd ships Docker, no sudo). It's
clone-based: each run gets its own fresh clone on a run branch, so Fabro owns the
branch → commit → push → PR lifecycle and runs can safely overlap.

The base image is **`freeq-fabro:tools`** (`.fabro/Dockerfile.tools`) — a slim
image with just the toolchain (rustfmt/clippy, cmake, libasound2-dev, Node,
cargo-audit), no baked source/target. Build/rebuild it on the VM:

```bash
boxd exec fabro-freeq -- 'cd ~/freeq && git pull && docker build -f .fabro/Dockerfile.tools -t freeq-fabro:tools .'
```

Runs set `CARGO_PROFILE_DEV_DEBUG=0` / `CARGO_PROFILE_TEST_DEBUG=0` (debug
symbols off). This doesn't change check/clippy/test correctness — it just keeps
the workspace build small and link-safe.

> **The executor is a fixed 2 vCPU / 8 GB / 98 GB boxd VM** (boxd can't resize),
> which is marginal for repeatedly building freeq's large Rust workspace. Two
> things this surfaced and how it's handled:
> - **debug=2 binaries blew the disk and bus-errored the linker** → `debug=0`
>   (above) keeps builds small/reliable. Each run cold-builds in ~20-30 min.
> - **Slow builds tempted the agent to yak-shave on build config** → every
>   implement/fix prompt now says: iterate with targeted `cargo test -p <crate>`,
>   run the full `.fabro/verify.sh` gate **once** at the end, and never touch
>   build config.
>
> **Follow-up (warmth):** a persistent host-mounted cache (`CARGO_TARGET_DIR` +
> registry as a docker volume) would make repeat builds incremental, once
> Fabro's volume-mount syntax is pinned down. Baking the target into the image
> is NOT the way (the layer commit needs ~2× disk and overflows the VM).
>
> **History:** the first setup used `provider = "local"` + `clone.enabled =
> false`, which bypassed Fabro's branch/PR machinery (work landed uncommitted on
> `main`). The docker provider fixed that.

## Running them

```bash
# One-off manual run (foreground), from the repo root on the executor:
fabro run .fabro/workflows/test-coverage/workflow.toml

# Background + just print the run id:
fabro run .fabro/workflows/bug-hunt/workflow.toml --detach

# Watch / inspect:
fabro events <run-id>      # event stream
fabro logs   <run-id>      # raw worker log
fabro inspect <run-id>     # status + stages
fabro artifact <run-id>    # screenshots / reports / traces
```

`--dry-run` executes with a simulated LLM (no spend, no real edits) to smoke the
wiring; `fabro preflight <workflow.toml>` validates config without executing.

## Going from manual → around the clock

The schedules are authored but **disabled**. After you've seen a PR you're happy
with, enable them by flipping `enabled = false → true` on the `schedule` trigger
in each [`automations/*.toml`](automations/) and committing — the server picks up
the change. (`api` triggers are already enabled, which is what manual/test runs
use.) Five-field cron, evaluated in **UTC**.

## Executor setup (one time)

See the provisioning notes the setup left in the VM, or rebuild from:

1. `boxd new --name=fabro-freeq --auto-suspend-timeout=0`
2. Install Fabro + a Rust/cmake/Node toolchain + `cargo-audit` on the VM.
3. `fabro secret set ANTHROPIC_API_KEY …` (+ any other providers) and the
   GitHub token; clone freeq to the working dir.
4. Run `fabro server` under systemd so it survives reboots/resumes.
5. The server reads these `.fabro/automations/*.toml` from `main` — keep this
   config on `main` so scheduled runs find it.
