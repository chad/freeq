import { useState, useEffect, useCallback } from 'react';
import { useStore } from '../store';
import { joinChannel, rawCommand } from '../irc/client';

interface RequirementStatus {
  requirement_type: string;
  description: string;
  satisfied: boolean;
  action?: {
    action_type: string;
    url?: string;
    label: string;
    accept_hash?: string;
  };
}

interface CheckResponse {
  channel: string;
  can_join: boolean;
  status: string;
  requirements: RequirementStatus[];
  role?: string;
}

export function JoinGateModal() {
  const channel = useStore((s) => s.joinGateChannel);
  const setChannel = useStore((s) => s.setJoinGateChannel);
  const authDid = useStore((s) => s.authDid);
  const [loading, setLoading] = useState(false);
  const [check, setCheck] = useState<CheckResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [accepting, setAccepting] = useState(false);
  const [joining, setJoining] = useState(false);

  const fetchCheck = useCallback(async () => {
    if (!channel || !authDid) return;
    setLoading(true);
    setError(null);
    try {
      const encoded = encodeURIComponent(channel);
      const res = await fetch(`/api/v1/policy/${encoded}/check`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ did: authDid }),
      });
      if (!res.ok) {
        const text = await res.text();
        setError(text || `HTTP ${res.status}`);
        return;
      }
      const data: CheckResponse = await res.json();
      setCheck(data);
    } catch (e: any) {
      setError(e.message || 'Failed to check requirements');
    } finally {
      setLoading(false);
    }
  }, [channel, authDid]);

  // Fetch on open
  useEffect(() => {
    if (channel) {
      setCheck(null);
      setError(null);
      setAccepting(false);
      setJoining(false);
      fetchCheck();
    }
  }, [channel, fetchCheck]);

  // Listen for credential verification completing (from popup)
  useEffect(() => {
    if (!channel) return;
    const handler = (e: MessageEvent) => {
      if (e.data?.type === 'freeq-credential' && e.data?.status === 'verified') {
        // Re-check requirements after credential was presented
        setTimeout(fetchCheck, 500);
      }
    };
    window.addEventListener('message', handler);
    return () => window.removeEventListener('message', handler);
  }, [channel, fetchCheck]);

  if (!channel) return null;

  const close = () => setChannel(null);

  const handleAccept = async (_hash: string) => {
    setAccepting(true);
    // Send POLICY ACCEPT via IRC (which auto-collects credentials)
    rawCommand(`POLICY ${channel} ACCEPT`);
    // Wait a moment then re-check
    setTimeout(fetchCheck, 1000);
    setTimeout(() => setAccepting(false), 1500);
  };

  const handleVerify = (url: string) => {
    let fullUrl = url;
    // Make relative URLs absolute
    if (!fullUrl.startsWith('http')) {
      fullUrl = `${window.location.origin}${fullUrl}`;
    }
    // Replace relative callback with absolute (the check endpoint returns relative paths)
    const absoluteCallback = `${window.location.origin}/api/v1/credentials/present`;
    fullUrl = fullUrl.replace(
      /callback=[^&]*/,
      `callback=${encodeURIComponent(absoluteCallback)}`
    );
    // Add callback if not present at all
    if (!fullUrl.includes('callback')) {
      const sep = fullUrl.includes('?') ? '&' : '?';
      fullUrl += `${sep}callback=${encodeURIComponent(absoluteCallback)}`;
    }
    // Add subject_did if not already in the URL
    if (!fullUrl.includes('subject_did') && authDid) {
      const sep = fullUrl.includes('?') ? '&' : '?';
      fullUrl += `${sep}subject_did=${encodeURIComponent(authDid)}`;
    }
    window.open(fullUrl, 'freeq-verify', 'width=600,height=700');
  };

  const handleJoin = () => {
    setJoining(true);
    joinChannel(channel);
    setTimeout(close, 500);
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/60 backdrop-blur-sm" onClick={close}>
      <div className="bg-bg-secondary border border-border rounded-xl shadow-2xl w-full max-w-md overflow-hidden" onClick={(e) => e.stopPropagation()}>
        {/* Header */}
        <div className="px-6 pt-5 pb-3 border-b border-border">
          <div className="flex items-center justify-between">
            <div>
              <h2 className="text-lg font-bold text-fg flex items-center gap-2">
                <span className="text-accent">#</span>
                {channel.replace(/^#/, '')}
              </h2>
              <p className="text-sm text-fg-dim mt-0.5">This channel requires policy acceptance to join</p>
            </div>
            <button onClick={close} className="text-fg-dim hover:text-fg p-1 -mr-1">
              <svg className="w-5 h-5" viewBox="0 0 20 20" fill="currentColor">
                <path fillRule="evenodd" d="M4.293 4.293a1 1 0 011.414 0L10 8.586l4.293-4.293a1 1 0 111.414 1.414L11.414 10l4.293 4.293a1 1 0 01-1.414 1.414L10 11.414l-4.293 4.293a1 1 0 01-1.414-1.414L8.586 10 4.293 5.707a1 1 0 010-1.414z" />
              </svg>
            </button>
          </div>
        </div>

        {/* Body */}
        <div className="px-6 py-4 max-h-96 overflow-y-auto">
          {loading && (
            <div className="flex items-center justify-center py-8">
              <div className="w-6 h-6 border-2 border-accent border-t-transparent rounded-full animate-spin" />
              <span className="ml-3 text-sm text-fg-dim">Checking requirements…</span>
            </div>
          )}

          {error && (
            <div className="bg-red-500/10 border border-red-500/20 rounded-lg p-3 text-sm text-red-400">
              {error}
            </div>
          )}

          {check && check.status === 'no_policy' && (
            <p className="text-sm text-fg-dim">This channel has no policy. You should be able to join directly.</p>
          )}

          {check && check.status === 'satisfied' && (
            <div className="text-center py-4">
              <div className="text-3xl mb-2">✅</div>
              <p className="text-fg font-medium">All requirements satisfied</p>
              {check.role && <p className="text-sm text-fg-dim mt-1">Role: <span className="text-accent font-mono">{check.role}</span></p>}
            </div>
          )}

          {check && check.requirements.length > 0 && (
            <div className="space-y-3">
              {check.requirements.map((req, i) => (
                <RequirementRow
                  key={i}
                  req={req}
                  accepting={accepting}
                  onAccept={handleAccept}
                  onVerify={handleVerify}
                />
              ))}
            </div>
          )}

          {check && check.can_join && check.role && check.requirements.length > 0 && (
            <div className="mt-4 p-3 bg-green-500/10 border border-green-500/20 rounded-lg text-sm text-green-400">
              ✓ All requirements met — role: <span className="font-mono font-bold">{check.role}</span>
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="px-6 py-4 border-t border-border flex items-center justify-between">
          <button onClick={close} className="text-sm text-fg-dim hover:text-fg">
            Cancel
          </button>
          <div className="flex gap-2">
            <button
              onClick={fetchCheck}
              className="px-3 py-1.5 text-sm text-fg-dim hover:text-fg border border-border rounded-lg hover:bg-bg-tertiary"
              title="Re-check requirements"
            >
              ↻ Refresh
            </button>
            <button
              onClick={handleJoin}
              disabled={!check?.can_join || joining}
              className={`px-4 py-1.5 rounded-lg text-sm font-medium transition-colors ${
                check?.can_join
                  ? 'bg-accent text-bg hover:bg-accent/90'
                  : 'bg-bg-tertiary text-fg-dim cursor-not-allowed'
              }`}
            >
              {joining ? 'Joining…' : 'Join Channel'}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function RequirementRow({
  req,
  accepting,
  onAccept,
  onVerify,
}: {
  req: RequirementStatus;
  accepting: boolean;
  onAccept: (hash: string) => void;
  onVerify: (url: string) => void;
}) {
  return (
    <div className={`flex items-start gap-3 p-3 rounded-lg border ${
      req.satisfied
        ? 'border-green-500/20 bg-green-500/5'
        : 'border-border bg-bg-tertiary'
    }`}>
      {/* Status icon */}
      <div className={`mt-0.5 w-5 h-5 rounded-full flex items-center justify-center shrink-0 ${
        req.satisfied ? 'bg-green-500/20 text-green-400' : 'bg-bg border border-border'
      }`}>
        {req.satisfied ? (
          <svg className="w-3 h-3" viewBox="0 0 12 12" fill="currentColor">
            <path d="M10 3L4.5 8.5 2 6" stroke="currentColor" strokeWidth="2" fill="none" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        ) : null}
      </div>

      {/* Content */}
      <div className="flex-1 min-w-0">
        <p className={`text-sm ${req.satisfied ? 'text-fg-muted' : 'text-fg'}`}>
          {req.description}
        </p>

        {/* Action button */}
        {!req.satisfied && req.action && (
          <div className="mt-2">
            {req.action.action_type === 'accept_rules' && req.action.accept_hash && (
              <button
                onClick={() => onAccept(req.action!.accept_hash!)}
                disabled={accepting}
                className="px-3 py-1.5 text-xs font-medium bg-accent text-bg rounded-md hover:bg-accent/90 disabled:opacity-50"
              >
                {accepting ? 'Accepting…' : req.action.label}
              </button>
            )}
            {req.action.action_type === 'verify_external' && req.action.url && (
              <button
                onClick={() => onVerify(req.action!.url!)}
                className="px-3 py-1.5 text-xs font-medium bg-bg border border-border text-fg rounded-md hover:bg-bg-secondary flex items-center gap-1.5"
              >
                {req.action.url!.includes('/bluesky/') ? (
                  <span>🦋</span>
                ) : (
                  <svg className="w-3.5 h-3.5" viewBox="0 0 16 16" fill="currentColor">
                    <path fillRule="evenodd" d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/>
                  </svg>
                )}
                {req.action.label}
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
