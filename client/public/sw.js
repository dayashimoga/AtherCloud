// AetherStream PWA Service Worker
const CACHE_NAME = "aetherstream-v1.0.0";
const SHELL_ASSETS = ["/", "/index.html", "/manifest.json"];

self.addEventListener("install", (evt) => {
  evt.waitUntil(
    caches.open(CACHE_NAME).then((cache) => cache.addAll(SHELL_ASSETS))
  );
  self.skipWaiting();
});

self.addEventListener("activate", (evt) => {
  evt.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(
        keys.map((k) => {
          if (k !== CACHE_NAME) return caches.delete(k);
        })
      )
    )
  );
  self.clients.claim();
});

self.addEventListener("fetch", (evt) => {
  // Do not cache API stream or dynamic transcode endpoints in Service Worker cache
  if (evt.request.url.includes("/api/v1/stream") || evt.request.url.includes("/api/v1/transcode")) {
    return;
  }

  evt.respondWith(
    caches.match(evt.request).then((cached) => cached || fetch(evt.request))
  );
});
