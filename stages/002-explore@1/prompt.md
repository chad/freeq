You are the lead engineer scoping a complex feature for the freeq codebase.

## The feature

harden and enhance file upload across the freeq server and web app, with on-freeq-server storage as the default

## Your job (this step is READ-ONLY — investigate, do not edit anything)

Map the current reality the feature touches, so the plan that follows is grounded in the actual code, not guesses.

1. **Find the surface.** Locate every place the feature concerns today across the Linux-verifiable codebase: `freeq-server` (esp. `src/web.rs`, media/upload, REST, `av_*`), `freeq-sdk` / `freeq-sdk-ffi`, the web app `freeq-app`, `freeq-tui`, and the protocol/docs (`docs/PROTOCOL.md`, `CLAUDE.md`). Use ripgrep/glob/read freely.
2. **Trace the end-to-end paths** the feature affects — request/response, storage, auth/capability, client rendering — and name the key files + functions.
3. **Capture constraints**: the protocol spec, existing tests, the CI gate (`.fabro/verify.sh` mirrors `ci.yml`), and anything in `CLAUDE.md` (hotspots, philosophy) that bounds the design.
4. **Threat model / failure modes** relevant to the feature (for uploads: size limits, content-type spoofing, path traversal, capability-URL leakage, quota, auth).
5. **Note the native clients** (`freeq-ios`, `freeq-macos`, `Freeq.WinUI`/`freeq-windows-app`) only enough to say what they'll *eventually* need — you will NOT edit them (this executor can't compile Swift/WinUI).

## Output

A concise but concrete findings brief: the current architecture of this surface, the files/functions that matter (with paths), the constraints, the threat model, and the open design questions the plan must answer. This brief is the foundation for the next step.