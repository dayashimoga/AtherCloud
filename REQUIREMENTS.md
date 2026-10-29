# SYSTEM REQUIREMENTS SPECIFICATION (SRS)
**Product:** AetherStream Media Streaming & Virtual Storage Platform  
**Status:** Canonical Engineering Specification  
**Version:** 1.0.0-PROD  

---

## 1. FUNCTIONAL REQUIREMENTS

### 1.1 Storage Virtualization & File Management
- **FR-101 (Virtual Mounts):** The system MUST support mounting arbitrary host directories and network shares into logical namespaces (e.g. `fast-nvme`, `movies-hdds`, `family-photos`) defined via declarative configuration files.
- **FR-102 (Directory Traversal Defense):** The system MUST block all relative directory navigation exploits (`../`, `..\\`, encoded `%2e%2e`, null-byte injections) and symlinks targeting files outside the configured mount root.
- **FR-103 (File Metadata Extraction):** The system MUST inspect and stream directory listings with file sizes, MIME types, modification timestamps, and media duration/codec summaries in `< 10ms` for directories containing up to 5,000 entries.
- **FR-104 (Scoped Share Links):** The system MUST generate cryptographic HMAC-signed links allowing granular read-only or read-write access to designated files or subfolders with configurable expiration timestamps.

### 1.2 Streaming & Content Delivery
- **FR-201 (HTTP Range Requests):** The server MUST implement RFC 7233 range requests returning `206 Partial Content` with `Content-Range` and `Accept-Ranges: bytes` headers for video seeking and resumption.
- **FR-202 (Adaptive Bitrate HLS Generation):** The media engine MUST dynamically generate HLS master and variant playlists (`.m3u8`) and segmented transport stream fragments (`.ts`) for $1080\text{p}$, $720\text{p}$, $480\text{p}$, and $360\text{p}$ profiles.
- **FR-203 (Direct Play Bypass):** If the client player natively supports the media container and codec (e.g. MP4 with H.264/AAC), the system MUST stream directly from disk via zero-copy I/O without invoking FFmpeg.
- **FR-204 (Hardware Acceleration Probing):** On startup and transcode scheduling, the engine MUST probe hardware encoders (NVENC, Intel QuickSync, VA-API, VideoToolbox) and automatically select the most performant available accelerator.

### 1.3 Client & Player Capabilities
- **FR-301 (Universal Responsive UI):** The client interface MUST render adaptively across mobile phones ($360\text{px}$ width), tablets, desktops, and 4K TV displays.
- **FR-302 (Live Telemetry HUD):** The video player MUST provide an overlay displaying real-time bandwidth (kbps), active rendition profile, dropped frame counts, forward buffer health (seconds), and latency.
- **FR-303 (Offline Media & PWA Caching):** The client MUST function as an installable Progressive Web App (PWA) with Service Worker offline shell caching and IndexedDB queueing for offline metadata inspection.
- **FR-304 (Cross-Platform Binaries):** The codebase MUST compile into native Android (`.apk`, `.aab`), iOS (`.ipa`, `.xcarchive`), and desktop binaries (`.exe`, `.AppImage`, `.dmg`).

### 1.4 Remote Ingress & Networking
- **FR-401 (Zero-Config NAT Punch-Through):** The system MUST integrate an automated tunnel orchestrator (Cloudflare Tunnel quick tunnels and named tunnels, Rathole VPS relay, or WireGuard) requiring zero port forwarding on residential NAT/CGNAT connections.

---

## 2. NON-FUNCTIONAL REQUIREMENTS & PERFORMANCE BUDGETS

### 2.1 Latency & Responsiveness
- **NFR-101 (Time-To-First-Frame - TTFF):** Direct stream playback MUST initiate within $\le 350\text{ ms}$ of player load on a $50\text{ Mbps}$ local connection. JIT transcoded streams MUST deliver the first playable fragment within $\le 800\text{ ms}$.
- **NFR-102 (API Response Time):** File tree and metadata API endpoints MUST respond with $p95 \le 15\text{ ms}$ and $p99 \le 40\text{ ms}$.
- **NFR-103 (Seek Latency):** Random video seek operations over HTTP range requests MUST resume playback in $\le 200\text{ ms}$.

### 2.2 System Resource Budgets
- **NFR-201 (Daemon Memory Footprint):** Idle server memory consumption MUST NOT exceed $25\text{ MB}$ RSS. Under active multi-stream load (10 concurrent sessions), server memory MUST NOT exceed $120\text{ MB}$ RSS (excluding FFmpeg process memory).
- **NFR-202 (Direct Stream CPU Overhead):** Direct stream read operations MUST consume $\le 1.5\%$ of a single CPU core per active $4\text{K}$ stream.
- **NFR-203 (Client Bundle Size):** The production PWA bundle (HTML + JS + CSS) MUST NOT exceed $450\text{ KiB}$ uncompressed ($140\text{ KiB}$ gzip/brotli compressed) excluding Hls.js core.

### 2.3 Reliability & Fault Tolerance
- **NFR-301 (Buffer Starvation Avoidance):** On fluctuating networks (bandwidth throttling down to $500\text{ kbps}$), the adaptive playback algorithm MUST drop renditions dynamically to sustain zero frame freezes and $\le 0.1\text{ stalls/hour}$.
- **NFR-302 (Transcode Process Isolation):** An FFmpeg process crash or segmenter failure MUST NOT crash the core daemon. The server MUST detect child process termination, log structured error diagnostics, and notify the client to retry or fall back.
- **NFR-303 (Orphaned File Sweeper):** Ephemeral HLS segments in scratch storage MUST be purged automatically after a 60-second inactivity TTL, guaranteeing zero disk leaks.

### 2.4 Security & Compliance
- **NFR-401 (Cryptographic Link Signatures):** HMAC tokens MUST utilize SHA-256 with timing-attack resistant verification (`constant_time_eq`).
- **NFR-402 (Zero Third-Party Tracking):** The client and server MUST NOT embed any third-party telemetry, ads, or external fonts/scripts, ensuring 100% self-contained data privacy.
- **NFR-403 (Safe Path Resolution):** Every filesystem query MUST enforce canonicalized root boundary validation prior to executing OS open/read syscalls.
