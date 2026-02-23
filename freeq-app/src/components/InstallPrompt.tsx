import { useState, useEffect } from 'react';

let deferredPrompt: any = null;

if (typeof window !== 'undefined') {
  window.addEventListener('beforeinstallprompt', (e) => {
    e.preventDefault();
    deferredPrompt = e;
  });
}

export function InstallPrompt() {
  const [show, setShow] = useState(false);
  const [dismissed, setDismissed] = useState(() => !!localStorage.getItem('freeq-install-dismissed'));

  useEffect(() => {
    // Show after 30 seconds if not dismissed and prompt available
    const timer = setTimeout(() => {
      if (deferredPrompt && !dismissed) setShow(true);
    }, 30000);
    return () => clearTimeout(timer);
  }, [dismissed]);

  if (!show || dismissed) return null;

  const install = async () => {
    if (!deferredPrompt) return;
    deferredPrompt.prompt();
    const result = await deferredPrompt.userChoice;
    if (result.outcome === 'accepted') {
      setShow(false);
    }
    deferredPrompt = null;
  };

  const dismiss = () => {
    setShow(false);
    setDismissed(true);
    localStorage.setItem('freeq-install-dismissed', '1');
  };

  return (
    <div className="fixed bottom-4 left-4 right-4 md:left-auto md:right-4 md:w-80 z-[100] bg-bg-secondary border border-border rounded-xl shadow-2xl p-4 animate-slideIn">
      <div className="flex items-start gap-3">
        <img src="/freeq.png" alt="" className="w-10 h-10 shrink-0" />
        <div className="flex-1 min-w-0">
          <div className="font-semibold text-sm text-fg">Install freeq</div>
          <div className="text-xs text-fg-dim mt-0.5">Get the full app experience with notifications and quick access.</div>
          <div className="flex gap-2 mt-3">
            <button onClick={install} className="bg-accent text-black text-xs font-bold px-4 py-1.5 rounded-lg hover:bg-accent-hover">
              Install
            </button>
            <button onClick={dismiss} className="text-xs text-fg-dim hover:text-fg-muted px-2">
              Not now
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
