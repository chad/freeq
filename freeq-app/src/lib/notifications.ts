/**
 * Browser notification system.
 * Handles permission, desktop notifications, title badge, and sounds.
 */

let enabled = true;
let soundEnabled = true;
let totalUnread = 0;
const originalTitle = document.title;

// Notification sound — tiny inline beep
let audioCtx: AudioContext | null = null;

function playSound() {
  if (!soundEnabled) return;
  try {
    if (!audioCtx) audioCtx = new AudioContext();
    const osc = audioCtx.createOscillator();
    const gain = audioCtx.createGain();
    osc.connect(gain);
    gain.connect(audioCtx.destination);
    osc.frequency.value = 800;
    osc.type = 'sine';
    gain.gain.setValueAtTime(0.1, audioCtx.currentTime);
    gain.gain.exponentialRampToValueAtTime(0.001, audioCtx.currentTime + 0.15);
    osc.start(audioCtx.currentTime);
    osc.stop(audioCtx.currentTime + 0.15);
  } catch { /* ignore */ }
}

export function setNotificationsEnabled(v: boolean) { enabled = v; }
export function setSoundEnabled(v: boolean) { soundEnabled = v; }

export async function requestPermission(): Promise<boolean> {
  if (!('Notification' in window)) return false;
  if (Notification.permission === 'granted') return true;
  const result = await Notification.requestPermission();
  return result === 'granted';
}

export function notify(title: string, body: string, onClick?: () => void) {
  if (!enabled) return;

  // Play sound
  playSound();

  // Update title badge
  totalUnread++;
  updateTitleBadge();

  // Desktop notification (only if page not focused)
  if (document.hidden && 'Notification' in window && Notification.permission === 'granted') {
    try {
      const n = new Notification(title, {
        body,
        icon: '/favicon.png',
        tag: 'freeq-' + title, // dedup per channel
      });
      if (onClick) {
        n.onclick = () => { window.focus(); onClick(); n.close(); };
      }
      setTimeout(() => n.close(), 5000);
    } catch { /* ignore */ }
  }
}

export function clearUnreadBadge() {
  totalUnread = 0;
  updateTitleBadge();
}

export function setUnreadCount(count: number) {
  totalUnread = count;
  updateTitleBadge();
}

function updateTitleBadge() {
  document.title = totalUnread > 0 ? `(${totalUnread}) ${originalTitle}` : originalTitle;
}

// Clear badge when window gains focus
if (typeof window !== 'undefined') {
  window.addEventListener('focus', () => {
    // Don't auto-clear — let the store manage this
  });
}
