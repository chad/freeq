// SDK wiring: connect via did:key SASL, then announce identity by
// sending PROVENANCE + AGENT REGISTER + initial presence + start
// the heartbeat loop. Returns the connected FreeqClient and a
// stop() that cleans up timers and quits.
//
// Phase 6 will hang the DM listener off the returned client.
import { FreeqClient } from "@freeq/sdk";
import type { AgentIdentity } from "./identity.js";
import type { Owner } from "./owner.js";
import type { DelegationCert } from "./delegation.js";

export interface ConnectOptions {
  identity: AgentIdentity;
  owner: Owner;
  delegation: DelegationCert;
  nick: string;
  /** Defaults to wss://irc.freeq.at/irc */
  serverUrl?: string;
  /** Heartbeat interval in ms; default 30_000. */
  heartbeatMs?: number;
}

export interface Connected {
  client: FreeqClient;
  /** The DID we authenticated as. */
  agentDid: string;
  /** The nick the server registered us with. */
  nick: string;
  /** Stops the heartbeat and sends QUIT. */
  stop(reason?: string): Promise<void>;
}

const DEFAULT_URL = "wss://irc.freeq.at/irc";

function b64urlEncode(s: string): string {
  return Buffer.from(s, "utf8")
    .toString("base64")
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

export async function connect(opts: ConnectOptions): Promise<Connected> {
  const url = opts.serverUrl ?? DEFAULT_URL;
  const heartbeatMs = opts.heartbeatMs ?? 30_000;

  const client = new FreeqClient({
    url,
    nick: opts.nick,
    sasl: {
      did: opts.identity.did,
      method: "crypto",
      signer: opts.identity.didKey.signer,
      token: "",
      pdsUrl: "",
    },
    // The agent already has a long-lived ed25519 identity (its did:key);
    // skip the SDK's auto-mint of a separate per-session MSGSIG key. The
    // server's resolve_signature() path will server-sign on our behalf
    // when we send messages without a client signature, which is fine
    // for v1.0. (Future: wire opts.identity.didKey through to a real
    // client signer.)
    autoMsgSig: false,
  });

  // ── Lifecycle: announce identity each time we become ready ──
  // SDK's auto-reconnect re-emits 'ready' after a successful resume,
  // so we always re-announce on every 'ready' (server state can be
  // gone after a server restart even if the SDK reconnected).
  let heartbeatTimer: NodeJS.Timeout | null = null;

  const announce = (): void => {
    if (heartbeatTimer) {
      clearInterval(heartbeatTimer);
      heartbeatTimer = null;
    }

    // Reclaim-rename: freeq's ghost-session reclaim adopts the previous
    // nick for multi-device continuity (registration.rs "Adopt the ghost's
    // nick"). If the user asked for a different nick this run, force the
    // rename now.
    if (client.nick && client.nick !== opts.nick) {
      client.raw(`NICK ${opts.nick}`);
    }

    // PROVENANCE: send the FreeqBotDelegation/v1 cert as base64url JSON.
    // The server's verify_provenance will recognize the type tag, look
    // up the creator's signing key (none for v1.0 unsigned certs), and
    // store the cert with _verified=false.
    const certJson = JSON.stringify(opts.delegation);
    client.raw(`PROVENANCE :${b64urlEncode(certJson)}`);

    // AGENT REGISTER: declare actor_class so the server (and other
    // clients) know we're an agent. WHOIS will include the class and
    // JOIN broadcasts will carry actor_class= as an IRCv3 tag.
    client.raw("AGENT REGISTER :class=agent");

    // PRESENCE: announce we're online and idle.
    client.raw("PRESENCE :state=online");

    // HEARTBEAT loop: 30s interval, 60s server-side TTL — matches the
    // documented Phase 1 default in agents.md and manifest.rs default.
    const beat = (): void => {
      try {
        client.raw("HEARTBEAT :state=active;ttl=60");
      } catch {
        // If the socket is gone the next reconnect will reissue from on('ready').
      }
    };
    beat(); // first beat right away
    heartbeatTimer = setInterval(beat, heartbeatMs);
  };

  // 'ready' fires after RPL_ENDOFMOTD (376) — the only reliable point to
  // start sending agent commands. PROVENANCE requires registered+authenticated.
  client.on("ready", announce);

  client.connect();

  // Wait for the SDK to confirm it's connected + registered. The SDK
  // emits 'ready' after MOTD which is what we need.
  await new Promise<void>((resolve, reject) => {
    const onReady = (): void => {
      cleanup();
      resolve();
    };
    const onError = (message: string): void => {
      cleanup();
      reject(new Error(message));
    };
    const cleanup = (): void => {
      client.off("ready", onReady);
      client.off("error", onError);
    };
    client.once("ready", onReady);
    client.once("error", onError);
  });

  const agentDid = opts.identity.did;
  const nick = client.nick || opts.nick;

  const stop = async (reason?: string): Promise<void> => {
    if (heartbeatTimer) {
      clearInterval(heartbeatTimer);
      heartbeatTimer = null;
    }
    try {
      client.raw("PRESENCE :state=offline");
      client.raw(reason ? `QUIT :${reason}` : "QUIT :freeqcc stop");
    } catch {
      // ignore — socket may already be gone
    }
    // Give the QUIT a moment to flush before tearing down.
    await new Promise((r) => setTimeout(r, 250));
    client.disconnect();
  };

  return { client, agentDid, nick, stop };
}
