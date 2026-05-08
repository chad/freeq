// Phase 2 — agent did:key identity. Stub until phase 2.
export interface AgentIdentity {
  did: string; // did:key:z…
  publicKey: Uint8Array; // 32 bytes
  privateKey: Uint8Array; // 32 bytes (ed25519 seed)
}

export async function loadOrCreateIdentity(): Promise<AgentIdentity> {
  throw new Error("not implemented (phase 2)");
}
