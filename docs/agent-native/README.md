# Agent-Native Implementation

This directory contains the detailed implementation plan for making Freeq an agent-forward coordination layer.

## Documents

| Document | Phase | What You Demo |
|---|---|---|
| [AGENT-NATIVE-IMPLEMENTATION.md](../AGENT-NATIVE-IMPLEMENTATION.md) | Overview | Architecture, data model, SDK changes, implementation order |
| [PHASE-1-KNOWN-ACTORS.md](PHASE-1-KNOWN-ACTORS.md) | Phase 1 | Agent badges, identity cards, provenance, live presence, heartbeat liveness |
| [PHASE-2-GOVERNABLE-AGENTS.md](PHASE-2-GOVERNABLE-AGENTS.md) | Phase 2 | TTL capabilities, pause/resume/revoke, deploy approval flows |
| [PHASE-3-COORDINATED-WORK.md](PHASE-3-COORDINATED-WORK.md) | Phase 3 | Structured task timelines, evidence attachments, audit trails |
| [PHASE-4-INTEROP-AND-SPAWNING.md](PHASE-4-INTEROP-AND-SPAWNING.md) | Phase 4 | Agent manifests, sub-agent spawning, MCP wrapper bridge |
| [PHASE-5-ECONOMIC-CONTROLS.md](PHASE-5-ECONOMIC-CONTROLS.md) | Phase 5 | Budget gauges, spend tracking, cost approval, budget limits |

## Existing Assets Leveraged

Each phase builds on what already works:

- **DID-based SASL auth** → agent identity (`did:plc` for humans, `did:web` and `did:key` for bots via `freeq-bot-id` tool)
- **ed25519 message signing** → action attestation
- **Policy engine** (PolicyDocument, requirements, attestations) → capability grants
- **GitHub/Bluesky verifiers** → provenance verification
- **away-notify + AWAY** → agent presence
- **freeq-bots** (factory, auditor, prototype) → demo agents
- **freeq-sdk** → agent SDK with FFI
- **iroh S2S federation** → federated agent presence and governance
- **Web client** (React) → agent UI

## Demo Story

The demo follows the factory bot through increasingly sophisticated scenarios:

1. **Phase 1**: Factory bot shows up with a 🤖 badge and a live identity card. Kill the process and watch it auto-degrade.
2. **Phase 2**: Factory bot needs approval to deploy. An op pauses it mid-build. It resumes when told to.
3. **Phase 3**: A full build produces a structured timeline with evidence at each stage. The audit tab traces every action.
4. **Phase 4**: Register the auditor from a manifest URL. Factory spawns a qa-worker sub-agent. An MCP agent joins through a wrapper.
5. **Phase 5**: The channel has a $50/day budget. Watch the gauge fill up. Bot gets blocked when it exceeds the limit.

Each phase's demo works on the web client AND on a legacy IRC client (irssi/weechat). The IRC client sees human-readable text versions of everything.

## Backwards Compatibility

All additions are IRCv3 vendor-namespaced tags (`+freeq.at/*`) and optional commands. Standard IRC clients connect and chat normally. The Freeq-native contract is progressive enhancement, not a gate.
