# freeq 1.0 / Public Beta Release Plan

_Last updated: 2026-06-11. Update this file as items complete._

## One-pager: what freeq is and why it matters

**freeq is an open, federated, agent-native communication substrate.** It takes
IRC — the only chat protocol that ever achieved true openness — and rebuilds it
for the present: AT Protocol (Bluesky) identity via a custom SASL mechanism
(`ATPROTO-CHALLENGE`), ed25519 message signing on every message, E2EE channels
and DMs (X3DH + Double Ratchet), server-to-server federation over iroh QUIC with
CRDT-converged state, voice/video over MoQ, and a first-class agent protocol:
cryptographic `did:key` identities, provenance certificates, governance
(pause/resume/revoke/budget), task coordination events, and agent spawning.

Clients: web (live at irc.freeq.at), iOS, macOS, Android, TUI — plus SDKs in
Rust and TypeScript, a bot-kit, and an agent-kit that bridges Claude Code into
live AV calls. Any legacy IRC client still connects in guest mode.

**Why it's impactful — three claims:**

1. **Slack and Discord are closed silos at exactly the moment conversation
   became the primary artifact of work.** "The conversation is the commit": when
   agents write the code, the human↔agent conversation *is* the engineering
   record. That record cannot live in a proprietary database with ephemeral
   retention and API rate limits. freeq makes conversations durable, signed,
   queryable, and owned by their participants — every message has a ULID msgid,
   a cryptographic signature, and a DID-bound author. Code history becomes
   decision history.

2. **Agents are second-class citizens everywhere else; here they are
   first-class.** On Slack/Discord a bot is an API token. On freeq an agent has
   a self-certifying DID, a provenance cert declaring who built it and on whose
   authority, governable lifecycle, budget controls, presence, and — via
   eliza/ghostly/revenant — a face, a voice, and persistent memory keyed to the
   DIDs of everyone it meets. Revenant demonstrates the category shift: an agent
   that sleeps for ~$0, wakes on summon in ~160ms, and remembers your last
   conversation. That is not a chatbot; it's a colleague with continuity.

3. **Open protocol + identity portability breaks the network-effect lock.**
   Identity is your AT Protocol DID, not an account in someone's database. Bans,
   ops, and reputation follow the DID across servers. Anyone can self-host and
   federate; the protocol is documented and IRCv3-compatible; the apps aim to be
   *better* than the closed equivalents, not a compromise you accept for
   ideology.

**Current state (honest):** ~1,230 commits in 4 months, 1,420 passing tests, a
46-bug security audit completed in March, live multi-month deployment, feature
parity across web/iOS/macOS, working voice/video, and two flagship agent
demos (freeqcc, revenant). The protocol and server are 1.0-grade. The gaps are
distribution, search, abuse-handling, notifications, and federation trust — the
things that only matter once strangers show up. Which is the point of a beta.

---

## Release strategy

Three audiences, one beta:

- **Developers** → SDKs/bot-kit/agent-kit published with versions, quickstarts, examples.
- **Self-hosters** → one-command deploy, backups, migrations, metrics, federation that's safe to open.
- **Public network users** → irc.freeq.at + web app + TestFlight, with search, push, and moderation.

Suggested sequence: **Beta blockers → public beta announcement → fast-follows → 1.0 gate.**

## Phase 0 — Release engineering (prerequisite for everything)

- [ ] Adopt semver; tag `v0.9.0-beta.1` (no git tags exist today)
- [ ] CHANGELOG.md generated from history; release notes discipline going forward
- [ ] GitHub Releases with prebuilt server binaries (linux x86_64/arm64, macOS)
- [ ] Docker image published to a registry (GHCR), pinned tags, `latest` = stable
- [ ] Publish `freeq-sdk` (Rust) to crates.io; pin/publish `@freeq/sdk`, `@freeq/bot-kit` versions on npm
- [ ] Homebrew formula for freeq-tui (and server)
- [ ] DB schema migration story: versioned migrations + tested upgrade path between releases (self-hosters will upgrade; today there is no documented path)

## Phase 1 — Public beta blockers

### Product credibility (Slack/Discord-replacement table stakes)
- [ ] **Search (FTS5)** — the #1 functional gap. Wire SQLite FTS5 to a SEARCH command + REST `/api/v1/search`; surface in web/macOS/iOS. Also fixes agent memory recall (revenant hit this; `fix/eliza-recall-or-semantics` branch exists — land it)
- [ ] **Real push notifications** — web push via service worker (currently only while tab open); APNs for iOS/macOS; FCM for Android. A team chat tool without notifications when closed is unusable as a daily driver
- [ ] **Message permalinks + export** — msgid-addressable URLs and a conversation export format. This is the "conversation is the commit" proof point; minimum viable: permalink + JSON/markdown export of a thread/channel range
- [ ] Web client offline/reconnect resilience pass (it has IndexedDB + SW; verify the disconnect/resume path under real network churn)

### Abuse & moderation (the public-network killer)
- [ ] Server-level admin tooling: oper commands to freeze/quarantine channels, shadow-limit guests, global DID ban
- [ ] Guest abuse posture: decide registration requirements for channel creation / DM initiation on the public network (DID-required is a reasonable default)
- [ ] Report flow (even minimal: /report → oper queue)
- [ ] Terms of service + moderation policy for irc.freeq.at; Code of Conduct in repo
- [ ] Moderation event log (CRDT, ULID-keyed) — at least the schema + append path, so beta moderation actions are auditable from day one

### Federation safety (strangers will federate during beta)
- [ ] **S2S mutual auth** — formalize: both directions check allowlist; document trust levels (Readonly/Relay/Full) and make them configurable per peer
- [ ] Fix topic merge flapping (SyncResponse vs CRDT strategy mismatch)
- [ ] Channel key removal (`-k`) propagation (additive-only SyncResponse gap)
- [ ] SyncResponse invite merge: add founder-authority check (open audit item)
- [ ] Federation operator guide: how to peer with the public network, what state you accept, how to de-peer

### Agent platform fixes (before pushing agent developers)
- [ ] **Ghost session bug**: DID session lingers after agent VM resume → crash loop (found by revenant). Implement server-side session takeover or timeout on duplicate DID auth
- [ ] Typing-indicator auto-timeout; backgroundWhois size cap; ConnectConfig validation (open audit items)
- [ ] Agent developer quickstart: "bot in 5 minutes" + "governed agent in 30" using bot-kit/agent-kit; publish freeqcc as the reference

### Self-hosting story — **miren.dev is the default path**
- [ ] Publish a Miren template/recipe for freeq: one command (or one click) → server + web client + auth broker + persistent volume for SQLite + TLS on your domain. Generalize `deploy/irc/deploy.sh` (today it's our bespoke tarball build) into something any Miren user can run against a release tag, not a source checkout
- [ ] Rewrite `docs/self-hosting.md` to lead with Miren; Docker Compose becomes the documented fallback, from-source last
- [ ] Smoke-test the Miren path end to end as a stranger would (fresh account, fresh domain, federate with irc.freeq.at)
- [ ] **Backup/restore documentation** (none exists; SQLite + WAL specifics — including what that means on Miren volumes)
- [ ] Prometheus `/metrics` endpoint (connections, messages, S2S peers, auth failures) — minimum viable observability
- [ ] Upgrade procedure doc tied to the migration story in Phase 0 (Miren redeploy + migration path included)

### Security posture
- [ ] SECURITY.md + responsible disclosure policy
- [ ] Add `cargo audit`/`npm audit` + a SAST pass to CI
- [ ] oauth.rs unit tests (mock PDS) — currently zero coverage on a security-critical path
- [ ] Key rotation: invalidate sessions when DID doc keys change (known limitation; matters more at public scale)

## Phase 2 — Beta launch

- [ ] iOS + macOS: public TestFlight links (privacy policy, App Store metadata prep)
- [ ] freeq.at site refresh: positioning per the one-pager, three audience funnels (use it / build on it / host it), docs reorganized for outsiders
- [ ] Announce: blog post pairing the launch with "The Conversation Is the Commit"; Bluesky-native launch (the identity layer *is* Bluesky — lean in)
- [ ] Seed the network: #freeq, #agents, #dev channels; run revenant personas publicly as the demo
- [ ] Status page + alerting for irc.freeq.at
- [ ] Feedback channel + triage cadence

## Phase 3 — 1.0 gate (after beta feedback)

- [ ] App Store / Play Store submissions (iOS, macOS; Android after small-screen polish)
- [ ] ATPROTO-CHALLENGE written up as an IRCv3 WG draft (credibility + the original acceptance criterion). Plan: extract a standalone normative spec from docs/PROTOCOL.md (crypto method normative; broker/PDS methods informative), reconcile spec-vs-implementation divergences (curve requirements, AUTHENTICATE 400-byte chunking, error numerics), open an ircv3-specifications issue → PR, offer a second implementation (e.g. Ergo extension) to satisfy the running-code norm, IANA SASL mechanism registration as a later step
- [ ] Second security review (external if possible) focused on federation + agent surfaces
- [ ] Conversation↔artifact linking: msgid trailers in git commits / freeqcc emitting commit↔conversation links (thesis feature, scope after beta learnings)
- [ ] TUI auto-reconnect
- [ ] sdk/client.rs connection state machine tests; irc/client.ts + MessageList.tsx unit tests (known undertested hotspots)
- [ ] Scale review of single-SQLite public server; document limits, plan sharding/read-replicas only if beta demand requires

## Explicitly deferred past 1.0

- Windows native app (design phase; web covers Windows)
- Screen share; browser P2P (iroh-live) rooms
- DID-based E2EE key exchange replacing passphrases; channel forward secrecy
- AT Protocol record-backed channels; label-based moderation integration
- Serverless P2P mode; reputation via social graph
- ghostly GPU backend / FFT audio bands / composite mode (revenant track, not core)

## What is already done (do not redo)

Protocol core, SASL/identity, signing, msgid, edit/delete/reactions/threads/pins,
CHATHISTORY, E2EE (channels + DM ratchet), S2S with CRDT + authz + rate limits,
agent phases 1–5 (identity/governance/tasks/spawning/economics), AV SFU
(voice+video, web+native), 46-bug security audit, 1,420 tests, CI, live
deployment, deploy scripts, Docker, web client at parity, iOS/macOS at parity.
