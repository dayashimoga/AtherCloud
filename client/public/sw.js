// AetherStream Universal PWA Service Worker (v1.2.0)
const CACHE_VERSION = "aetherstream-v1.2.0";
const SHELL_CACHE_NAME = `${CACHE_VERSION}-shell`;
const RUNTIME_CACHE_NAME = `${CACHE_VERSION}-runtime`;

// Pre-cached App Shell Assets & Icons
const SHELL_ASSETS = [
  "/",
  "/index.html",
  "/manifest.json",
  "/icon-192.png",
  "/icon-512.png"
];

// 1. Install Phase: Pre-cache App Shell & Icons
self.addEventListener("install", (evt) => {
  evt.waitUntil(
    caches.open(SHELL_CACHE_NAME).then(async (cache) => {
      console.log("[PWA-SW] Pre-caching core app shell & icon assets...");
      try {
        await cache.addAll(SHELL_ASSETS);
      } catch (err) {
        console.warn("[PWA-SW] Non-fatal pre-caching asset warning:", err);
      }
    })
  );
  self.skipWaiting();
});

// 2. Activate Phase: Purge Outdated Cache Generations & Claim Clients
self.addEventListener("activate", (evt) => {
  evt.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(
        keys.map((key) => {
          if (key !== SHELL_CACHE_NAME && key !== RUNTIME_CACHE_NAME) {
            console.log("[PWA-SW] Purging outdated cache generation:", key);
            return caches.delete(key);
          }
        })
      )
    )
  );
  self.clients.claim();
});

// 3. Message Listener: Support skipWaiting for immediate client activation
self.addEventListener("message", (evt) => {
  if (evt.data && evt.data.type === "SKIP_WAITING") {
    self.skipWaiting();
  }
});

// 4. Fetch Strategy & Request Interception
self.addEventListener("fetch", (evt) => {
  const req = evt.request;
  const url = new URL(req.url);

  // Bypass non-GET requests (e.g. POST, PUT, DELETE, PATCH)
  if (req.method !== "GET") {
    return;
  }

  // Never cache live streaming segments, dynamic transcode, chunked uploads, or websockets
  if (
    url.pathname.includes("/api/v1/stream") ||
    url.pathname.includes("/api/v1/transcode") ||
    url.pathname.includes("/api/v1/hls") ||
    url.pathname.includes("/api/v1/upload") ||
    url.pathname.includes("/api/v1/ws")
  ) {
    return;
  }

  // Strategy A: Navigation requests (HTML / SPA App Shell fallback)
  if (req.mode === "navigate") {
    evt.respondWith(
      fetch(req).catch(async () => {
        console.log("[PWA-SW] Offline navigation, returning cached app shell:", req.url);
        const cached = await caches.match("/index.html");
        return cached || (await caches.match("/"));
      })
    );
    return;
  }

  // Strategy B: API endpoints (Network-first with runtime cache fallback)
  if (url.pathname.startsWith("/api/")) {
    evt.respondWith(
      fetch(req)
        .then((resp) => {
          if (resp && resp.status === 200) {
            const clone = resp.clone();
            caches.open(RUNTIME_CACHE_NAME).then((cache) => cache.put(req, clone));
          }
          return resp;
        })
        .catch(() => caches.match(req))
    );
    return;
  }

  // Strategy C: Static Assets (JS, CSS, Images, Icons, Fonts)
  // Stale-While-Revalidate with Cache-First Fallback
  evt.respondWith(
    caches.match(req).then((cached) => {
      const fetchPromise = fetch(req)
        .then((networkResp) => {
          if (networkResp && networkResp.status === 200) {
            const clone = networkResp.clone();
            caches.open(RUNTIME_CACHE_NAME).then((cache) => cache.put(req, clone));
          }
          return networkResp;
        })
        .catch((err) => {
          // If offline and requesting an icon or image, fallback to cached icon
          if (url.pathname.endsWith(".png") || url.pathname.endsWith(".ico")) {
            return caches.match("/icon-192.png");
          }
          throw err;
        });

      return cached || fetchPromise;
    })
  );
});
