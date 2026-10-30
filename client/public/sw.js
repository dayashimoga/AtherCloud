// AetherStream PWA Service Worker
const CACHE_NAME = "aetherstream-v1.0.0";
const SHELL_ASSETS = [
  "/",
  "/index.html",
  "/manifest.json",
  "/icon-192.png",
  "/icon-512.png"
];

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
  // Never cache live streaming segments, dynamic transcode, or chunked upload endpoints
  if (
    evt.request.url.includes("/api/v1/stream") ||
    evt.request.url.includes("/api/v1/transcode") ||
    evt.request.url.includes("/api/v1/upload")
  ) {
    return;
  }

  // Network-first for other API calls, cache fallback
  if (evt.request.url.includes("/api/")) {
    evt.respondWith(
      fetch(evt.request).catch(() => caches.match(evt.request))
    );
    return;
  }

  // Cache-first for static shell assets
  evt.respondWith(
    caches.match(evt.request).then((cached) => cached || fetch(evt.request))
  );
});
