import { useState, useEffect } from 'react';
import { fetchProfile, type ATProfile } from '../lib/profiles';
import { useStore } from '../store';
import { sendWhois } from '../irc/client';
import * as e2ee from '../lib/e2ee';

function ProvenanceBlock({ provenance }: { provenance: NonNullable<ActorInfo['provenance']> }) {
  const [creatorProfile, setCreatorProfile] = useState<ATProfile | null>(null);
  // Nick lookup for the creator. did:plc resolves via AT Protocol profile;
  // did:key has no profile, so we fall through to the actor-identity REST
  // endpoint which returns the creator's current IRC nick if it has an
  // active session. Without this, sub-agent cards render their creator as
  // a raw "did:key:z6Mk…" string instead of a recognizable name like
  // "lobot" — observed live with society panelists owned by the moderator.
  const [creatorNick, setCreatorNick] = useState<string | null>(null);
  useEffect(() => {
    if (!provenance.creator_did) return;
    const did = provenance.creator_did;
    const isDidKey = did.startsWith('did:key:');
    if (!isDidKey) {
      fetchProfile(did).then((p) => p && setCreatorProfile(p));
    }
    fetch(`/api/v1/actors/${encodeURIComponent(did)}`)
      .then((r) => (r.ok ? r.json() : null))
      .then((j) => {
        if (j?.nick) setCreatorNick(j.nick);
      })
      .catch(() => {});
  }, [provenance.creator_did]);

  const creatorLabel = creatorProfile
    ? creatorProfile.displayName || creatorProfile.handle
    : creatorNick || provenance.creator_did;

  return (
    <div className="mt-2 p-2 bg-bg-tertiary rounded-lg text-left">
      <div className="text-[10px] text-fg-dim font-semibold mb-1">Provenance</div>
      {provenance.creator_did && (
        <div className="text-[10px] text-fg-dim flex items-center gap-1.5">
          <span className="text-fg-dim/60">Creator:</span>
          <button
            onClick={() => { navigator.clipboard.writeText(provenance.creator_did!); import('./Toast').then(m => m.showToast('DID copied', 'success', 2000)); }}
            title="Click to copy DID"
            className="flex items-center gap-1 cursor-pointer hover:opacity-80"
          >
            {creatorProfile?.avatar && (
              <img src={creatorProfile.avatar} alt="" className="w-3.5 h-3.5 rounded-full" />
            )}
            <span className="text-fg-muted">{creatorLabel}</span>
          </button>
        </div>
      )}
      {provenance.source_repo && (
        <div className="text-[10px] text-fg-dim">
          <span className="text-fg-dim/60">Source:</span>{' '}
          <a href={provenance.source_repo} target="_blank" rel="noopener noreferrer" className="text-accent hover:underline">
            {provenance.source_repo.replace('https://', '')}
          </a>
        </div>
      )}
      {provenance.implementation_ref && (
        <div className="text-[10px] text-fg-dim">
          <span className="text-fg-dim/60">Impl:</span>{' '}
          <span className="font-mono">{provenance.implementation_ref}</span>
        </div>
      )}
    </div>
  );
}

interface ActorInfo {
  actor_class?: string;
  did?: string;
  online?: boolean;
  spawned?: boolean;
  parent_did?: string;
  parent_nick?: string;
  channel?: string;
  capabilities?: string[];
  ttl?: number;
  task?: string;
  provenance?: {
    creator_did?: string;
    source_repo?: string;
    implementation_ref?: string;
    revocation_authority?: string;
    origin_type?: string;
    authority_basis?: string;
  };
  presence?: {
    state?: string;
    status?: string;
    task?: string;
  };
  heartbeat?: {
    last_seen?: string;
    ttl?: number;
    healthy?: boolean;
  };
}

interface UserPopoverProps {
  nick: string;
  did?: string;
  position: { x: number; y: number };
  onClose: () => void;
}

export function UserPopover({ nick, did, position, onClose }: UserPopoverProps) {
  const [profile, setProfile] = useState<ATProfile | null>(null);
  const [loading, setLoading] = useState(false);
  const setActive = useStore((s) => s.setActiveChannel);
  const addChannel = useStore((s) => s.addChannel);
  const whois = useStore((s) => s.whoisCache.get(nick.toLowerCase()));
  const [safetyNumber, setSafetyNumber] = useState<string | null>(null);
  const [actorInfo, setActorInfo] = useState<ActorInfo | null>(null);

  useEffect(() => {
    // Always trigger WHOIS to get latest info
    sendWhois(nick);
  }, [nick]);

  const effectiveDid = did || whois?.did;
  const isDidKey = effectiveDid?.startsWith('did:key:');

  // Fetch safety number for E2EE verification
  useEffect(() => {
    if (effectiveDid && e2ee.hasSession(effectiveDid)) {
      e2ee.getSafetyNumber(effectiveDid).then(setSafetyNumber);
    }
  }, [effectiveDid]);

  // Fetch AT profile when we have a DID (skip did:key — they have no Bluesky profile)
  useEffect(() => {
    if (effectiveDid && !isDidKey && !profile) {
      setLoading(true);
      fetchProfile(effectiveDid).then((p) => {
        setProfile(p);
        setLoading(false);
      });
    } else if (isDidKey) {
      setLoading(false);
    }
  }, [effectiveDid]);

  // Fetch actor info from REST API (agent class, provenance, presence)
  // Try by DID first, fall back to nick (for spawned agents before WHOIS completes)
  useEffect(() => {
    const fetchActor = async () => {
      if (effectiveDid) {
        const r = await fetch(`/api/v1/actors/${encodeURIComponent(effectiveDid)}`);
        if (r.ok) { setActorInfo(await r.json()); return; }
      }
      // Fallback: try by nick (spawned agents may not have DID yet)
      const r2 = await fetch(`/api/v1/actors/${encodeURIComponent(nick)}`);
      if (r2.ok) { setActorInfo(await r2.json()); }
    };
    fetchActor().catch(() => {});
  }, [effectiveDid, nick]);

  const startDM = () => {
    addChannel(nick);
    setActive(nick);
    onClose();
  };

  // Position keeping on screen
  const style: React.CSSProperties = {
    position: 'fixed',
    left: Math.min(position.x, window.innerWidth - 300),
    top: Math.min(position.y, window.innerHeight - 400),
    zIndex: 100,
  };

  const displayName = profile?.displayName || whois?.realname || nick;
  const handle = profile?.handle || whois?.handle;
  const avatarUrl = profile?.avatar;

  return (
    <>
      <div className="fixed inset-0 z-40" onClick={onClose} />
      <div style={style} className="z-50 bg-bg-secondary border border-border rounded-xl shadow-2xl w-72 animate-fadeIn overflow-hidden">
        {/* Header */}
        <div className="h-16 bg-gradient-to-r from-accent/20 to-purple/20 relative">
          {avatarUrl ? (
            <img
              src={avatarUrl}
              alt=""
              className="absolute -bottom-6 left-4 w-14 h-14 rounded-full border-4 border-bg-secondary object-cover"
            />
          ) : (
            <div className="absolute -bottom-6 left-4 w-14 h-14 rounded-full border-4 border-bg-secondary bg-surface flex items-center justify-center text-accent font-bold text-lg">
              {nick[0]?.toUpperCase()}
            </div>
          )}
        </div>

        <div className="pt-8 px-4 pb-4">
          {/* Display name */}
          <div className="font-semibold text-fg">{displayName}</div>
          {displayName !== nick && (
            <div className="text-sm text-fg-muted">{nick}</div>
          )}

          {/* AT Handle — only for AT Protocol users (not did:key) */}
          {handle && !isDidKey && (
            <div className="text-xs text-accent mt-1 flex items-center gap-1">
              <span>@{handle}</span>
              <span className="text-success text-[10px]" title="AT Protocol identity">✓</span>
            </div>
          )}

          {/* Agent badge */}
          {actorInfo && (actorInfo.actor_class === 'agent' || actorInfo.actor_class === 'external_agent') && (
            <div className="inline-flex items-center gap-1 mt-1 px-2 py-0.5 bg-accent/10 rounded-full text-xs text-accent">
              🤖 {actorInfo.spawned ? 'Spawned Agent' : actorInfo.actor_class === 'external_agent' ? 'External Agent' : 'Agent'}
            </div>
          )}

          {/* Spawned agent info */}
          {actorInfo?.spawned && (
            <div className="mt-2 p-2 bg-bg-tertiary rounded-lg text-left">
              <div className="text-[10px] text-fg-dim font-semibold mb-1">Spawned Agent</div>
              {actorInfo.parent_nick && (
                <div className="text-[10px] text-fg-dim">
                  <span className="text-fg-dim/60">Parent:</span>{' '}
                  <span className="font-semibold text-fg-muted">{actorInfo.parent_nick}</span>
                </div>
              )}
              {actorInfo.task && (
                <div className="text-[10px] text-fg-dim">
                  <span className="text-fg-dim/60">Task:</span> {actorInfo.task}
                </div>
              )}
              {actorInfo.capabilities && actorInfo.capabilities.length > 0 && (
                <div className="text-[10px] text-fg-dim">
                  <span className="text-fg-dim/60">Caps:</span> {actorInfo.capabilities.join(', ')}
                </div>
              )}
              {actorInfo.ttl && (
                <div className="text-[10px] text-fg-dim">
                  <span className="text-fg-dim/60">TTL:</span> {actorInfo.ttl}s
                </div>
              )}
            </div>
          )}

          {/* DID */}
          {effectiveDid && (
            <div
              className="text-[10px] text-fg-dim mt-1 font-mono break-all cursor-pointer hover:text-fg-muted"
              onClick={() => { navigator.clipboard.writeText(effectiveDid); import('./Toast').then(m => m.showToast('DID copied', 'success', 2000)); }}
              title="Click to copy DID"
            >
              {effectiveDid}
            </div>
          )}

          {/* Agent presence */}
          {actorInfo?.presence && actorInfo.presence.state && (
            <div className="mt-2 p-2 bg-bg-tertiary rounded-lg text-left">
              <div className="text-[10px] text-fg-dim font-semibold mb-1">Presence</div>
              <div className="text-xs text-fg-muted flex items-center gap-1">
                <span>{
                  { online: '🟢', idle: '💤', active: '⚡', executing: '🔨',
                    waiting_for_input: '⏳', blocked_on_permission: '🔒',
                    blocked_on_budget: '💰', degraded: '🟡', paused: '⏸️',
                    sandboxed: '📦', rate_limited: '🚦', revoked: '🚫', offline: '⚫',
                  }[actorInfo.presence.state] || '•'
                }</span>
                <span>{actorInfo.presence.state}</span>
              </div>
              {actorInfo.presence.status && (
                <div className="text-[10px] text-fg-dim mt-0.5">{actorInfo.presence.status}</div>
              )}
            </div>
          )}

          {/* Provenance */}
          {actorInfo?.provenance && (
            <ProvenanceBlock provenance={actorInfo.provenance} />
          )}

          {/* Heartbeat */}
          {actorInfo?.heartbeat && (
            <div className="mt-2 p-2 bg-bg-tertiary rounded-lg text-left">
              <div className="text-[10px] text-fg-dim font-semibold mb-1">Heartbeat</div>
              <div className="text-[10px] text-fg-dim flex items-center gap-1">
                {actorInfo.heartbeat.healthy ? (
                  <span className="text-success">💓 healthy</span>
                ) : (
                  <span className="text-error">💔 unhealthy</span>
                )}
                {actorInfo.heartbeat.ttl && <span>· TTL {actorInfo.heartbeat.ttl}s</span>}
              </div>
            </div>
          )}

          {/* E2EE Safety Number */}
          {safetyNumber && (
            <div className="mt-2 p-2 bg-success/5 border border-success/20 rounded-lg">
              <div className="text-[10px] text-success font-semibold mb-1 flex items-center gap-1">
                🔒 Encrypted DM — Safety Number
              </div>
              <div className="text-[10px] font-mono text-fg-dim leading-relaxed tracking-wider">
                {safetyNumber}
              </div>
              <div className="text-[9px] text-fg-dim mt-1">
                Compare with your contact to verify encryption
              </div>
            </div>
          )}

          {/* Bio */}
          {profile?.description && (
            <div className="text-xs text-fg-muted mt-2 leading-relaxed line-clamp-3">
              {profile.description}
            </div>
          )}

          {/* WHOIS info (for guests or extra detail) */}
          {whois && (
            <div className="mt-2 space-y-0.5">
              {whois.user && whois.host && (
                <div className="text-[11px] text-fg-dim font-mono">
                  {whois.user}@{whois.host}
                </div>
              )}
              {whois.channels && (
                <div className="text-[11px] text-fg-dim">
                  <span className="text-fg-dim">Channels:</span> {whois.channels}
                </div>
              )}
              {whois.server && (
                <div className="text-[11px] text-fg-dim">
                  <span className="text-fg-dim">Server:</span> {whois.server}
                </div>
              )}
            </div>
          )}

          {loading && !profile && !whois && (
            <div className="text-xs text-fg-dim mt-2 flex items-center gap-1">
              <svg className="animate-spin w-3 h-3" viewBox="0 0 24 24">
                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" fill="none" />
                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
              </svg>
              Loading...
            </div>
          )}

          {/* No identity badge for guests */}
          {!effectiveDid && !loading && whois && (
            <div className="text-[10px] text-fg-dim mt-2 bg-bg-tertiary rounded px-2 py-1">
              Guest — no AT Protocol identity
            </div>
          )}

          {/* Actions */}
          <div className="flex gap-2 mt-3">
            <button
              onClick={startDM}
              className="flex-1 bg-accent/10 hover:bg-accent/20 text-accent text-xs py-1.5 rounded-lg font-medium"
            >
              Message
            </button>
            {handle && !isDidKey && (
              <a
                href={`https://bsky.app/profile/${handle}`}
                target="_blank"
                rel="noopener noreferrer"
                className="flex-1 bg-bg-tertiary hover:bg-surface text-fg-muted hover:text-fg text-xs py-1.5 rounded-lg text-center"
              >
                Bluesky ↗
              </a>
            )}
          </div>
        </div>
      </div>
    </>
  );
}
