You are an independent **compatibility reviewer** critiquing a proposed implementation plan. Find what breaks; don't praise.

## The feature

harden and enhance file upload across the freeq server and web app, with on-freeq-server storage as the default

## What to do

Read `/tmp/plan.md` and check it against the real code. Critique through a **protocol- and cross-client-compatibility lens**:

- **Protocol/wire compatibility**: are changes additive and versioned? Does anything break the IRC/TAGMSG/REST contract in `docs/PROTOCOL.md`? Will older clients that don't understand the new fields still work (graceful degradation)?
- **Native clients we are NOT editing now** (`freeq-ios`, `freeq-macos`, Windows app): does the server/web contract let them keep functioning unchanged, then adopt the feature later without a flag day? Is the follow-up section accurate about what they'll need?
- **SDK surface**: are `freeq-sdk` / `freeq-sdk-ffi` changes backward-compatible? FFI ABI stability?
- **Web app**: does `freeq-app` degrade gracefully if the server is older/newer?
- **Migration**: existing uploads/data keep resolving.

## Output

Write your critique to **`/tmp/review-compat.md`** (the synthesizer reads it
from there — this is how your review reaches it), and also print it as your
final message.

A focused critique: each compatibility break or risk, who it affects, and the specific plan change required. Confirm the scope fence holds (no native-client edits planned). Be concrete. Do not edit the plan (`/tmp/plan.md`) or any code.