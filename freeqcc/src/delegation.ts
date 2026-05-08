// Phase 4 — FreeqBotDelegation/v1 cert. Shells out to the existing freeq-bot-id Rust CLI.
// Stub until phase 4.
export interface DelegationCert {
  type: "FreeqBotDelegation/v1";
  agent_did: string;
  creator_did: string;
  scope: string[];
  issued_at: string;
  expires_at: string | null;
  revocation_uri: string | null;
  proof: {
    type: string;
    verificationMethod: string;
    signatureValue: string;
  };
}

export async function loadDelegation(): Promise<DelegationCert | null> {
  throw new Error("not implemented (phase 4)");
}

export async function mintDelegation(_args: {
  ownerDid: string;
  agentDid: string;
}): Promise<DelegationCert> {
  throw new Error("not implemented (phase 4)");
}
