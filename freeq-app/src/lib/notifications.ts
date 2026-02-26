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
    const t = audioCtx.currentTime;

    // Two-tone chime (C5 → E5)
    const notes = [523.25, 659.25];
    notes.forEach((freq, i) => {
      const osc = audioCtx!.createOscillator();
      const gain = audioCtx!.createGain();
      osc.connect(gain);
      gain.connect(audioCtx!.destination);
      osc.frequency.value = freq;
      osc.type = 'sine';
      const start = t + i * 0.08;
      gain.gain.setValueAtTime(0, start);
      gain.gain.linearRampToValueAtTime(0.08, start + 0.01);
      gain.gain.exponentialRampToValueAtTime(0.001, start + 0.2);
      osc.start(start);
      osc.stop(start + 0.2);
    });
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

// Favicon badge rendering
const originalFavicon = document.querySelector<HTMLLinkElement>('link[rel="icon"]')?.href || '/favicon.png';
let faviconCanvas: HTMLCanvasElement | null = null;

function updateFaviconBadge() {
  const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
  if (!link) return;

  if (totalUnread <= 0) {
    link.href = originalFavicon;
    return;
  }

  if (!faviconCanvas) {
    faviconCanvas = document.createElement('canvas');
    faviconCanvas.width = 32;
    faviconCanvas.height = 32;
  }

  const img = new Image();
  img.crossOrigin = 'anonymous';
  img.onload = () => {
    const ctx = faviconCanvas!.getContext('2d');
    if (!ctx) return;
    ctx.clearRect(0, 0, 32, 32);
    ctx.drawImage(img, 0, 0, 32, 32);
    // Red dot
    ctx.beginPath();
    ctx.arc(24, 8, 8, 0, 2 * Math.PI);
    ctx.fillStyle = '#ff5c5c';
    ctx.fill();
    // Count text (if small enough)
    if (totalUnread <= 99) {
      ctx.font = 'bold 11px sans-serif';
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillStyle = '#fff';
      ctx.fillText(totalUnread > 9 ? '9+' : String(totalUnread), 24, 8.5);
    }
    link.href = faviconCanvas!.toDataURL('image/png');
  };
  img.src = originalFavicon;
}

function updateTitleBadge() {
  document.title = totalUnread > 0 ? `(${totalUnread}) ${originalTitle}` : originalTitle;
  updateFaviconBadge();
}

// Clear badge when window gains focus
if (typeof window !== 'undefined') {
  window.addEventListener('focus', () => {
    // Don't auto-clear — let the store manage this
  });
}
