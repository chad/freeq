/**
 * Dynamically loads the MoQ web component scripts (moq-publish, moq-watch).
 *
 * These are custom elements built from the moq repository, served as static
 * assets by freeq-server at /av/assets/. They're loaded on demand when the
 * user first joins a call, not at page load.
 */

let loaded = false;
let loading: Promise<void> | null = null;

const SCRIPTS = [
  '/av/assets/publish-Du5ksDQe.js',
  '/av/assets/watch-CTz_Tjt7.js',
];

const PRELOADS = [
  '/av/assets/time-D4Xqna_f.js',
];

export function loadMoqComponents(): Promise<void> {
  if (loaded) return Promise.resolve();
  if (loading) return loading;

  loading = new Promise<void>((resolve, reject) => {
    let remaining = SCRIPTS.length;
    let failed = false;

    // The moq-publish and moq-watch scripts auto-initialize on load and
    // expect their elements to already exist in the DOM. Add hidden
    // placeholder elements so the scripts don't throw "missing element".
    // CallPanel will create its own elements later; these are just for init.
    if (!document.querySelector('moq-publish')) {
      const placeholder = document.createElement('moq-publish');
      placeholder.style.display = 'none';
      placeholder.id = '__moq-placeholder-pub';
      document.body.appendChild(placeholder);
    }
    if (!document.querySelector('moq-watch')) {
      const placeholder = document.createElement('moq-watch');
      placeholder.style.display = 'none';
      // moq-watch needs a canvas child
      const canvas = document.createElement('canvas');
      canvas.style.display = 'none';
      placeholder.appendChild(canvas);
      placeholder.id = '__moq-placeholder-watch';
      document.body.appendChild(placeholder);
    }

    // Preload shared dependencies
    for (const href of PRELOADS) {
      if (!document.querySelector(`link[href="${href}"]`)) {
        const link = document.createElement('link');
        link.rel = 'modulepreload';
        link.crossOrigin = '';
        link.href = href;
        document.head.appendChild(link);
      }
    }

    // Load the main scripts
    for (const src of SCRIPTS) {
      if (document.querySelector(`script[src="${src}"]`)) {
        remaining--;
        if (remaining === 0) { loaded = true; resolve(); }
        continue;
      }

      const script = document.createElement('script');
      script.type = 'module';
      script.crossOrigin = '';
      script.src = src;
      script.onload = () => {
        remaining--;
        if (remaining === 0 && !failed) { loaded = true; resolve(); }
      };
      script.onerror = () => {
        if (!failed) {
          failed = true;
          loading = null;
          reject(new Error(`Failed to load MoQ script: ${src}`));
        }
      };
      document.head.appendChild(script);
    }

    if (remaining === 0) { loaded = true; resolve(); }
  });

  return loading;
}

/** Check if moq-publish custom element is registered */
export function isMoqLoaded(): boolean {
  return loaded && typeof customElements !== 'undefined' && !!customElements.get('moq-publish');
}
