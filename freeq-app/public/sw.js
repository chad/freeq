// Service worker — offline shell + push notifications (stub)
const CACHE_NAME = 'freeq-v1';
const SHELL_URLS = ['/', '/index.html'];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) => cache.addAll(SHELL_URLS))
  );
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(keys.filter((k) => k !== CACHE_NAME).map((k) => caches.delete(k)))
    )
  );
  self.clients.claim();
});

self.addEventListener('fetch', (event) => {
  // Network-first for API/WS, cache-first for static assets
  const url = new URL(event.request.url);
  if (url.pathname.startsWith('/irc') || url.pathname.startsWith('/api') || url.pathname.startsWith('/auth')) {
    return; // Don't cache API requests
  }
  event.respondWith(
    fetch(event.request).catch(() => caches.match(event.request))
  );
});
