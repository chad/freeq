// Phase 3 — owner config (Bluesky handle + PLC-resolved DID). Stub until phase 3.
export interface Owner {
  handle: string;
  did: string; // did:plc:…
  pubkeyJwkB64?: string; // optional cached creator pubkey for cert verification
}

export async function loadOwner(): Promise<Owner | null> {
  throw new Error("not implemented (phase 3)");
}

export async function promptAndStoreOwner(): Promise<Owner> {
  throw new Error("not implemented (phase 3)");
}
