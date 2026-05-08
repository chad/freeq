// Phase 5 — SDK wiring: SASL did:key, PROVENANCE <cert>, AGENT REGISTER, presence + heartbeat.
// Stub until phase 5.
import type { AgentIdentity } from "./identity.js";
import type { Owner } from "./owner.js";
import type { DelegationCert } from "./delegation.js";

export interface ConnectOptions {
  identity: AgentIdentity;
  owner: Owner;
  delegation: DelegationCert;
  nick: string;
  serverUrl?: string; // default wss://irc.freeq.at/irc
}

export async function connect(_opts: ConnectOptions): Promise<unknown> {
  throw new Error("not implemented (phase 5)");
}
