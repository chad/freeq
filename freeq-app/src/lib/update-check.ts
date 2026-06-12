/**
 * Stale-tab detector.
 *
 * SPA tabs left open across a deploy keep running the old bundle forever —
 * `index.html` is served `Cache-Control: no-cache`, so a *reload* always
 * heals, but nothing ever told the user to reload. (2026-06-12: a five-day-old
 * tab ran pre-camera-fix code and published audio-only calls — "the bot can't
 * see me" — while the fix had been live for days.)
 *
 * Strategy: remember which `/assets/index-*.js` bundle this tab booted from,
 * then re-fetch `/` (no-store) on visibility-gain and every POLL_MS. When the
 * served hash differs, show a persistent toast with a Reload action. Never
 * auto-reload — the user may be mid-call or mid-compose.
 */
import { showToast } from '../components/Toast';

const BUNDLE_RE = /\/assets\/(index-[\w-]+)\.js/;
const POLL_MS = 5 * 60 * 1000;

/** Pull the JS bundle name (e.g. `index-BqhcCn03`) out of an index.html body. */
export function extractBundleHash(html: string): string | null {
  return html.match(BUNDLE_RE)?.[1] ?? null;
}

/** The bundle this tab actually loaded, from the live <script type="module"> tag. */
export function loadedBundleHash(doc: Document = document): string | null {
  const script = doc.querySelector<HTMLScriptElement>('script[type="module"][src*="/assets/index-"]');
  return script ? extractBundleHash(script.src) : null;
}

async function servedBundleHash(): Promise<string | null> {
  try {
    const res = await fetch('/', { cache: 'no-store' });
    if (!res.ok) return null;
    return extractBundleHash(await res.text());
  } catch {
    return null; // offline / mid-deploy — try again next tick
  }
}

/**
 * Start watching for deploys. Returns a cleanup function.
 * No-op in dev (Vite serves `/src/main.tsx`, no hashed bundle to compare).
 */
export function startUpdateCheck(): () => void {
  const loaded = loadedBundleHash();
  if (!loaded) return () => {};

  // One toast per detection; re-arm when the tab goes hidden so a user who
  // dismissed it gets re-prompted on their next return — stale tabs that
  // sit in the background for days are exactly the failure mode.
  let notified = false;

  const check = async () => {
    if (notified) return;
    const served = await servedBundleHash();
    if (!served || served === loaded) return;
    notified = true;
    showToast(
      'A new version of freeq is available.',
      'info',
      0, // persistent until acted on or dismissed
      { label: 'Reload', onClick: () => window.location.reload() },
    );
  };

  const onVisibility = () => {
    if (document.visibilityState === 'visible') void check();
    else notified = false;
  };

  document.addEventListener('visibilitychange', onVisibility);
  const interval = window.setInterval(() => void check(), POLL_MS);

  return () => {
    document.removeEventListener('visibilitychange', onVisibility);
    window.clearInterval(interval);
  };
}
