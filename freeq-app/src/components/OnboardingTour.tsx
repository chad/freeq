import { useState, useEffect } from 'react';
import { useStore } from '../store';

const LS_KEY = 'freeq-onboarding-done';

const STEPS = [
  {
    title: 'Welcome to freeq! 🎉',
    body: "freeq is IRC reimagined with AT Protocol identity. Your Bluesky login is your chat identity — portable, verifiable, yours.",
    icon: '🌐',
  },
  {
    title: 'Channels & DMs',
    body: 'Channels start with # — join as many as you like. Click any user to start a private conversation.',
    icon: '💬',
  },
  {
    title: 'Your Identity Matters',
    body: "Signed in with AT Protocol? You get a verified badge (✓), your Bluesky avatar, and cryptographic proof of who you are.",
    icon: '✓',
  },
  {
    title: 'Power Features',
    body: '⌘K to quick-switch channels. ⌘F to search. Right-click messages for actions. Drag files to upload. Type / for commands.',
    icon: '⚡',
  },
  {
    title: 'Invite Your Friends',
    body: 'Right-click any channel → Copy invite link. Share it on Bluesky, Twitter, or anywhere. They can join with one click.',
    icon: '🔗',
  },
];

export function OnboardingTour() {
  const registered = useStore((s) => s.registered);
  const [step, setStep] = useState(0);
  const [show, setShow] = useState(false);

  useEffect(() => {
    if (registered && !localStorage.getItem(LS_KEY)) {
      // Delay slightly so user sees the app first
      const t = setTimeout(() => setShow(true), 1500);
      return () => clearTimeout(t);
    }
  }, [registered]);

  if (!show) return null;

  const current = STEPS[step];
  const isLast = step === STEPS.length - 1;

  const finish = () => {
    setShow(false);
    localStorage.setItem(LS_KEY, '1');
  };

  return (
    <div className="fixed inset-0 z-[500] flex items-center justify-center bg-black/60 backdrop-blur-sm">
      <div className="bg-bg-secondary border border-border rounded-2xl shadow-2xl w-[420px] max-w-[92vw] overflow-hidden animate-fadeIn">
        {/* Progress dots */}
        <div className="flex justify-center gap-1.5 pt-5">
          {STEPS.map((_, i) => (
            <div key={i} className={`w-2 h-2 rounded-full transition-colors ${
              i === step ? 'bg-accent' : i < step ? 'bg-accent/40' : 'bg-border'
            }`} />
          ))}
        </div>

        <div className="px-8 pt-6 pb-8 text-center">
          <div className="text-4xl mb-4">{current.icon}</div>
          <h2 className="text-xl font-bold text-fg mb-2">{current.title}</h2>
          <p className="text-sm text-fg-muted leading-relaxed">{current.body}</p>

          <div className="flex gap-3 justify-center mt-8">
            {step > 0 && (
              <button
                onClick={() => setStep(step - 1)}
                className="text-sm text-fg-dim hover:text-fg-muted px-4 py-2"
              >
                Back
              </button>
            )}
            {isLast ? (
              <button
                onClick={finish}
                className="bg-accent text-black font-bold text-sm px-8 py-2.5 rounded-xl hover:bg-accent-hover"
              >
                Let's go! 🚀
              </button>
            ) : (
              <button
                onClick={() => setStep(step + 1)}
                className="bg-accent text-black font-bold text-sm px-8 py-2.5 rounded-xl hover:bg-accent-hover"
              >
                Next
              </button>
            )}
            {!isLast && (
              <button
                onClick={finish}
                className="text-sm text-fg-dim hover:text-fg-muted px-4 py-2"
              >
                Skip
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
