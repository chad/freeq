import { useEffect, useRef, useCallback, useState } from 'react';
import { useStore } from '../store';
import { getAvInstanceId, getNick, leaveAvSession } from '../irc/client';
import { loadMoqComponents } from '../lib/moq-loader';
import { getCachedProfile } from '../lib/profiles';

/**
 * Inline call panel with audio + video support.
 *
 * Camera is OFF by default (audio only). When any participant turns on their
 * camera, the panel expands to show a video grid. Participants with camera off
 * show their avatar or initials.
 *
 * Uses moq-publish `invisible` attribute to control camera:
 * - invisible set → camera off (audio only)
 * - invisible removed → camera on (video + audio)
 */
export function CallPanel() {
  const activeAvSession = useStore((s) => s.activeAvSession);
  const avAudioActive = useStore((s) => s.avAudioActive);
  const avMuted = useStore((s) => s.avMuted);
  const avCameraOn = useStore((s) => s.avCameraOn);
  const avSessions = useStore((s) => s.avSessions);

  const session = activeAvSession ? avSessions.get(activeAvSession) : null;
  const sessionId = session?.id;
  const channel = session?.channel;

  const publishContainerRef = useRef<HTMLDivElement>(null);
  const localVideoRef = useRef<HTMLVideoElement>(null);
  const publishElRef = useRef<HTMLElement | null>(null);
  const pollTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const localStreamRef = useRef<MediaStream | null>(null);

  const [participantSlots, setParticipantSlots] = useState<Slot[]>([]);
  // Full-screen: the call panel takes over the whole web-app viewport so
  // participant video (and utopia's visual-aid cards) is actually big
  // enough to see.
  const [fullscreen, setFullscreen] = useState(false);

  const myNick = getNick();
  // Connect to the SFU's QUIC/WebTransport listener (udp :8080) rather
  // than MoQ-over-WebSocket. WebTransport is the proper real-time media
  // transport; the WebSocket path degrades into static under publish
  // load. The `https://` scheme tells moq-publish/moq-watch to use
  // WebTransport. See docs/AV-QUIC-MIGRATION.md.
  const moqOrigin = `https://${location.hostname}:8080/av/moq`;

  // ── Start/stop call when avAudioActive changes ──────────────
  useEffect(() => {
    if (!avAudioActive || !sessionId || !myNick) return;
    let cancelled = false;

    async function start() {
      try {
        await loadMoqComponents();
      } catch (e) {
        console.error('[call] Failed to load MoQ components:', e);
        useStore.getState().addSystemMessage(channel || 'server', 'Failed to load audio components');
        useStore.getState().setAvAudioActive(false);
        return;
      }
      if (cancelled) return;

      // Request mic permission (camera handled separately on toggle)
      // We only need the permission — stop the stream immediately so it
      // doesn't interfere with moq-publish's own getUserMedia call.
      try {
        const permStream = await navigator.mediaDevices.getUserMedia({ audio: true });
        permStream.getTracks().forEach((t) => t.stop());
      } catch (e: unknown) {
        const err = e as { name?: string; message?: string };
        const reason = err.name === 'NotAllowedError' ? 'microphone permission denied'
          : err.name === 'NotFoundError' ? 'no microphone found'
          : err.message || 'unknown error';
        console.error('[call] Mic error:', reason);
        useStore.getState().addSystemMessage(channel || 'server', `Microphone error: ${reason}`);
        useStore.getState().setAvAudioActive(false);
        return;
      }
      if (cancelled) return;

      const container = publishContainerRef.current;
      if (!container) return;

      const pub = document.createElement('moq-publish');
      container.appendChild(pub);
      publishElRef.current = pub;

      // Include the per-call instance suffix the IRC layer generated for
      // our av-join TAGMSG so this device's path is unique even if the
      // same DID is also publishing from another tab/device.
      const myInstance = getAvInstanceId();
      const broadcastName = myInstance
        ? `${sessionId}/${myNick}~${myInstance}`
        : `${sessionId}/${myNick}`;
      pub.setAttribute('url', moqOrigin);
      pub.setAttribute('name', broadcastName);
      pub.setAttribute('source', 'camera');
      // Camera off by default
      pub.setAttribute('invisible', '');
      console.log('[call] Publishing:', broadcastName);

      pollParticipants();
      pollTimerRef.current = setInterval(pollParticipants, 3000);
    }

    start();
    return () => { cancelled = true; cleanup(); };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [avAudioActive, sessionId]);

  // ── Sync mute state ─────────────────────────────────────────
  useEffect(() => {
    const pub = publishElRef.current;
    if (!pub) return;
    // Belt + suspenders: set both the DOM attribute and the JS property
    // — moq-publish's mute implementation has shifted between attribute-
    // observed and property-observed at various versions, and silently
    // ignoring one half of that contract surfaces as "the icon toggles
    // but my voice still goes through".
    if (avMuted) {
      pub.setAttribute('muted', '');
    } else {
      pub.removeAttribute('muted');
    }
    (pub as HTMLElement & { muted?: boolean }).muted = avMuted;
  }, [avMuted]);

  // ── Sync camera state ───────────────────────────────────────
  useEffect(() => {
    const pub = publishElRef.current;
    if (!pub) return;

    if (avCameraOn) {
      pub.removeAttribute('invisible');
      // Start local preview
      navigator.mediaDevices.getUserMedia({ video: true, audio: false })
        .then((stream) => {
          localStreamRef.current = stream;
          if (localVideoRef.current) {
            localVideoRef.current.srcObject = stream;
          }
        })
        .catch((e) => {
          console.warn('[call] Camera error:', e);
          useStore.getState().setAvCameraOn(false);
        });
    } else {
      pub.setAttribute('invisible', '');
      // Stop local preview
      if (localStreamRef.current) {
        localStreamRef.current.getTracks().forEach((t) => t.stop());
        localStreamRef.current = null;
      }
      if (localVideoRef.current) {
        localVideoRef.current.srcObject = null;
      }
    }
  }, [avCameraOn]);

  // ── Poll participants ───────────────────────────────────────
  const pollParticipants = useCallback(async () => {
    if (!sessionId) return;
    try {
      const resp = await fetch(`/api/v1/sessions/${encodeURIComponent(sessionId)}`);
      if (!resp.ok) return;
      const data = await resp.json();
      if (!data.participants) return;

      const myInstance = getAvInstanceId();

      // Each participant slot is identified by (nick, instance_id). Two
      // devices on the same DID return two entries with the same nick but
      // different instance_id — and we have to subscribe to each path
      // independently. The watcher map is keyed by the full broadcast key
      // so the per-slot lifecycle works.
      const slots: Slot[] = data.participants
        .filter((p: { nick: string; instance_id?: string | null }) => {
          // Skip *our own* slot (matching nick AND matching instance_id).
          if (p.nick.toLowerCase() !== myNick.toLowerCase()) return true;
          if (myInstance && p.instance_id && p.instance_id === myInstance) return false;
          if (!myInstance && !p.instance_id) return false;
          // Same nick, different instance — that's another device of ours.
          // Subscribe so the user hears themselves across devices (useful
          // for verifying the call is wired up at all).
          return true;
        })
        .map((p: { nick: string; instance_id?: string | null }) => {
          const broadcastKey = p.instance_id ? `${p.nick}~${p.instance_id}` : p.nick;
          const broadcastName = `${sessionId}/${broadcastKey}`;
          return { nick: p.nick, broadcastKey, broadcastName };
        });

      console.log(
        '[call] poll: participants=%o myInstance=%s slots=%o',
        data.participants,
        myInstance,
        slots.map((s) => s.broadcastKey),
      );

      // Replace the slot list in state. The actual moq-watch element for
      // each slot is mounted inside its tile by RemoteTile via a ref
      // callback — no more invisible container.
      setParticipantSlots((prev) => {
        const sameLen = prev.length === slots.length;
        const sameKeys =
          sameLen && prev.every((p, i) => p.broadcastKey === slots[i].broadcastKey);
        return sameKeys ? prev : slots;
      });
    } catch (e) {
      console.warn('[call] Poll failed:', e);
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, myNick, moqOrigin]);

  // ── Cleanup ─────────────────────────────────────────────────
  function cleanup() {
    if (pollTimerRef.current) {
      clearInterval(pollTimerRef.current);
      pollTimerRef.current = null;
    }
    const pub = publishElRef.current;
    if (pub) {
      pub.setAttribute('source', '');
      pub.setAttribute('url', '');
      pub.remove();
      publishElRef.current = null;
    }
    if (localStreamRef.current) {
      localStreamRef.current.getTracks().forEach((t) => t.stop());
      localStreamRef.current = null;
    }
    setParticipantSlots([]);
  }

  const handleMuteToggle = () => useStore.getState().setAvMuted(!avMuted);
  const handleCameraToggle = () => useStore.getState().setAvCameraOn(!avCameraOn);
  const handleLeave = () => {
    cleanup();
    useStore.getState().setAvAudioActive(false);
    useStore.getState().setAvCameraOn(false);
    if (channel && sessionId) leaveAvSession(channel, sessionId);
  };

  if (!avAudioActive || !sessionId) return null;

  const participantCount = (session?.participants.size || 0);
  const showVideoGrid = avCameraOn || participantSlots.length > 0;
  const authDid = useStore.getState().authDid;
  const myAvatar = authDid ? getCachedProfile(authDid)?.avatar : null;

  return (
    <div
      className={
        fullscreen
          ? 'fixed inset-0 z-40 bg-bg-secondary flex flex-col'
          : 'border-b border-border bg-bg-secondary'
      }
    >
      {/* Video grid — shown when camera is on or participants exist */}
      {showVideoGrid && (
        <div
          className={
            fullscreen
              ? 'flex-1 flex flex-wrap gap-4 p-4 justify-center items-center content-center overflow-y-auto'
              : 'flex flex-wrap gap-2 p-2 justify-center max-h-64 overflow-y-auto'
          }
        >
          {/* Local tile */}
          <div className={tileClasses(fullscreen)}>
            {avCameraOn ? (
              <video
                ref={localVideoRef}
                autoPlay
                muted
                playsInline
                className="w-full h-full object-cover mirror"
                style={{ transform: 'scaleX(-1)' }}
              />
            ) : (
              <AvatarTile name={myNick} avatarUrl={myAvatar} />
            )}
            <span className="absolute bottom-1 left-1 text-[10px] bg-black/60 text-white px-1 rounded">
              You {avMuted && '(muted)'}
            </span>
          </div>

          {/* Remote tiles — one moq-watch per participant slot, mounted
              inside its own visible container (was previously rendered
              into a hidden div, so video subscriptions worked but never
              reached the screen). */}
          {participantSlots.map((slot) => (
            <RemoteTile
              key={slot.broadcastKey}
              slot={slot}
              moqOrigin={moqOrigin}
              fullscreen={fullscreen}
            />
          ))}
        </div>
      )}

      {/* Controls bar */}
      <div className="flex items-center gap-3 px-4 py-2">
        <div className="flex items-center gap-1.5 text-success font-medium text-sm">
          <span className="w-2.5 h-2.5 rounded-full bg-success animate-pulse" />
          <span>{avCameraOn ? 'Video' : 'Voice'} ({participantCount})</span>
        </div>

        <div className="flex-1" />

        {/* Mute */}
        <button
          onClick={handleMuteToggle}
          className={`p-2 rounded-full transition-colors ${
            avMuted
              ? 'bg-danger text-white hover:bg-danger/80'
              : 'bg-bg-tertiary text-fg hover:bg-bg-tertiary/80'
          }`}
          title={avMuted ? 'Unmute' : 'Mute'}
        >
          {avMuted ? <MicOffIcon size={18} /> : <MicIcon size={18} />}
        </button>

        {/* Camera */}
        <button
          onClick={handleCameraToggle}
          className={`p-2 rounded-full transition-colors ${
            avCameraOn
              ? 'bg-accent text-white hover:bg-accent/80'
              : 'bg-bg-tertiary text-fg hover:bg-bg-tertiary/80'
          }`}
          title={avCameraOn ? 'Turn off camera' : 'Turn on camera'}
        >
          {avCameraOn ? <CameraOnIcon size={18} /> : <CameraOffIcon size={18} />}
        </button>

        {/* Full screen */}
        <button
          onClick={() => setFullscreen((f) => !f)}
          className="p-2 rounded-full bg-bg-tertiary text-fg hover:bg-bg-tertiary/80 transition-colors"
          title={fullscreen ? 'Exit full screen' : 'Full screen'}
        >
          {fullscreen ? <MinimizeIcon size={18} /> : <MaximizeIcon size={18} />}
        </button>

        {/* Leave */}
        <button
          onClick={handleLeave}
          className="p-2 rounded-full bg-danger text-white hover:bg-danger/80 transition-colors"
          title="Leave call"
        >
          <PhoneOffIcon size={18} />
        </button>
      </div>

      {/* Hidden containers for moq elements */}
      <div ref={publishContainerRef} className="hidden" />
    </div>
  );
}

/** Shows avatar or initials when camera is off */
type Slot = { nick: string; broadcastKey: string; broadcastName: string };

/// Remote participant tile that mounts its own `<moq-watch>` element so
/// video actually appears on the screen. The avatar sits underneath
/// as a fallback when the participant hasn't enabled their camera.
/// Tile sizing — tiny thumbnails inline, large 16:9 tiles in full
/// screen (16:9 so utopia's video isn't cropped).
function tileClasses(fullscreen: boolean): string {
  return fullscreen
    ? 'relative w-[42vw] max-w-[820px] min-w-[280px] aspect-video rounded-xl overflow-hidden bg-bg-tertiary flex-shrink-0'
    : 'relative w-32 h-24 rounded-lg overflow-hidden bg-bg-tertiary flex-shrink-0';
}

function RemoteTile({
  slot,
  moqOrigin,
  fullscreen,
}: {
  slot: Slot;
  moqOrigin: string;
  fullscreen: boolean;
}) {
  const mountRef = useRef<HTMLDivElement>(null);
  const profile = getCachedProfile(slot.nick);

  useEffect(() => {
    const mount = mountRef.current;
    if (!mount) return;
    const watchEl = document.createElement('moq-watch');
    const canvas = document.createElement('canvas');
    canvas.className = 'absolute inset-0 w-full h-full object-cover';
    watchEl.appendChild(canvas);
    watchEl.style.position = 'absolute';
    watchEl.style.inset = '0';
    watchEl.style.width = '100%';
    watchEl.style.height = '100%';
    // 80ms jitter buffer — a middle ground. 30ms was too tight: it
    // underran on normal decode/network jitter and left audible static
    // in the audio. 80ms still beats moq-watch's ~100ms default (keeps
    // calls snappy) while giving the buffer enough slack for clean
    // audio. Raise toward 100ms+ if stutter shows up on bad networks.
    watchEl.setAttribute('jitter', '80');
    watchEl.setAttribute('url', moqOrigin);
    watchEl.setAttribute('name', slot.broadcastName);
    mount.appendChild(watchEl);
    console.log('[call] Subscribing to:', slot.broadcastName);

    return () => {
      watchEl.setAttribute('url', '');
      watchEl.remove();
    };
  }, [slot.broadcastName, moqOrigin]);

  return (
    <div className={tileClasses(fullscreen)}>
      <AvatarTile name={slot.nick} avatarUrl={profile?.avatar} />
      <div ref={mountRef} className="absolute inset-0" />
      <span className="absolute bottom-1 left-1 text-[10px] bg-black/60 text-white px-1 rounded z-10">
        {slot.nick}
      </span>
    </div>
  );
}

function MaximizeIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
      <path d="M2 6V2h4M14 6V2h-4M2 10v4h4M14 10v4h-4" />
    </svg>
  );
}

function MinimizeIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
      <path d="M6 2v4H2M10 2v4h4M6 14v-4H2M10 14v-4h4" />
    </svg>
  );
}

function AvatarTile({ name, avatarUrl }: { name: string; avatarUrl?: string | null }) {
  const initials = name.slice(0, 2).toUpperCase();
  return (
    <div className="w-full h-full flex items-center justify-center bg-bg-tertiary">
      {avatarUrl ? (
        <img src={avatarUrl} alt={name} className="w-12 h-12 rounded-full object-cover" />
      ) : (
        <div className="w-12 h-12 rounded-full bg-accent/20 flex items-center justify-center text-accent font-bold text-lg">
          {initials}
        </div>
      )}
    </div>
  );
}

function MicIcon({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor">
      <path d="M3.5 6.5A.5.5 0 0 1 4 7v1a4 4 0 0 0 8 0V7a.5.5 0 0 1 1 0v1a5 5 0 0 1-4.5 4.975V15h3a.5.5 0 0 1 0 1h-7a.5.5 0 0 1 0-1h3v-2.025A5 5 0 0 1 3 8V7a.5.5 0 0 1 .5-.5z"/>
      <path d="M10 8a2 2 0 1 1-4 0V3a2 2 0 1 1 4 0v5zM8 0a3 3 0 0 0-3 3v5a3 3 0 0 0 6 0V3a3 3 0 0 0-3-3z"/>
    </svg>
  );
}

function MicOffIcon({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor">
      <path d="M13 8c0 .564-.094 1.107-.266 1.613l-.814-.814A4.02 4.02 0 0 0 12 8V7a.5.5 0 0 1 1 0v1zm-5 4c.818 0 1.578-.245 2.212-.667l.718.719a4.973 4.973 0 0 1-2.43.923V15h3a.5.5 0 0 1 0 1h-7a.5.5 0 0 1 0-1h3v-2.025A5 5 0 0 1 3 8V7a.5.5 0 0 1 1 0v1a4 4 0 0 0 4 4zm3-9v4.879L5.158 2.037A3.001 3.001 0 0 1 11 3z"/>
      <path d="M9.486 10.607 5 6.12V8a3 3 0 0 0 4.486 2.607zm-7.84-1.96-.001-.001 1.442-1.442-.001-.001L14.96.33l.708.707L1.354 15.354l-.707-.707L4.14 11.153A4.985 4.985 0 0 1 3 8V7a.5.5 0 0 1 1 0v1c0 .455.076.897.216 1.306l.59-.59A4.02 4.02 0 0 1 4 8z"/>
    </svg>
  );
}

function CameraOnIcon({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor">
      <path fillRule="evenodd" d="M0 5a2 2 0 0 1 2-2h7.5a2 2 0 0 1 1.983 1.738l3.11-1.382A1 1 0 0 1 16 4.269v7.462a1 1 0 0 1-1.406.913l-3.111-1.382A2 2 0 0 1 9.5 13H2a2 2 0 0 1-2-2V5z"/>
    </svg>
  );
}

function CameraOffIcon({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor">
      <path fillRule="evenodd" d="M10.961 12.365a1.99 1.99 0 0 0 .522-1.103l3.11 1.382A1 1 0 0 0 16 11.731V4.269a1 1 0 0 0-1.406-.913l-3.111 1.382A2 2 0 0 0 9.5 3H4.272l6.69 9.365zm-10.114-9A2 2 0 0 0 0 5v6a2 2 0 0 0 2 2h5.728L.847 3.366zm9.746 11.925-14-19 .646-.708 14 19-.646.708z"/>
    </svg>
  );
}

function PhoneOffIcon({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor">
      <path d="M10.68 4.236a.4.4 0 0 0-.358-.221H5.68a.4.4 0 0 0-.358.221L3.566 7.7a.4.4 0 0 0 .036.407l1.571 2.16-.426.733a.4.4 0 0 0 .047.444l1.602 1.837a.4.4 0 0 0 .603 0l1.602-1.837a.4.4 0 0 0 .047-.444l-.426-.733 1.571-2.16a.4.4 0 0 0 .036-.407L10.68 4.236z" transform="rotate(135 8 8)"/>
    </svg>
  );
}
