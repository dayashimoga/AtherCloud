# AetherStream

> Ultra-low-latency, zero-cost personal media streaming and virtual storage ecosystem engineered in Rust and TypeScript.

[![Rust](https://img.shields.io/badge/rust-1.78%2B-orange.svg)](https://www.rust-lang.org/)
[![Axum](https://img.shields.io/badge/axum-0.7-blue.svg)](https://github.com/tokio-rs/axum)
[![TypeScript](https://img.shields.io/badge/typescript-5.4-blue.svg)](https://www.typescriptlang.org/)
[![Vite](https://img.shields.io/badge/vite-5.4-purple.svg)](https://vitejs.dev/)
[![Status](https://img.shields.io/badge/milestones-100%25%20complete-brightgreen.svg)](state.tracker.json)

---

## Overview

AetherStream transforms personal hardware into a self-hosted, multi-gigabit media streaming platform and virtual file cloud. By combining an asynchronous Rust backend with a hardware-accelerated transcoding pipeline and a cross-platform client shell, AetherStream delivers Netflix-grade adaptive bitrate (ABR) video streaming and Google Drive-grade file management with zero subscription fees and zero cloud vendor lock-in.

---

## Key Architectural Capabilities

### 1. High-Throughput Storage & I/O Engine
- **Zero-Copy Streaming:** `ReaderStream` async socket pipes achieving **27.9 Gbps** loopback throughput and sub-10ms Time to First Byte (TTFB).
- **RFC 7233 Range Requests:** Full byte-range seeking support with minimal memory overhead.
- **SQLite WAL Virtual Mount Table:** Granular Role-Based Access Control (`Read`, `Write`, `StreamOnly`) with canonical path sandboxing and symlink escape defenses.
- **Resumable Uploads:** Chunked 2 MiB slice uploads with client and server Web Crypto SHA-256 integrity verification.

### 2. Hardware-Accelerated Adaptive Video Pipeline
- **Auto-Detection Ladder:** Dynamic hardware acceleration fallback:  
  `NVIDIA NVENC` $\to$ `Intel QuickSync (QSV)` $\to$ `Apple VideoToolbox` $\to$ `Linux VA-API` $\to$ `libx264 CPU fallback`.
- **RFC 8216 Dynamic HLS Generator:** On-the-fly master playlist compilation with multi-rendition ladders (1080p, 720p, 480p, 360p).
- **JIT Segmenter & Ring Buffer:** 4-second keyframe-aligned HLS chunking with Least-Recently-Used (LRU) disk cache eviction.
- **Universal Audio Processing:** Dynamic Dolby Pro Logic II (DPLII) 5.1/7.1 downmixing to stereo AAC for seamless browser compatibility.

### 3. Universal Client UI & Player
- **Responsive Web UI:** Fast, mobile-first file explorer with grid/list toggles, instant client-side filtering, and clickable breadcrumbs.
- **Adaptive Video Player:** Fine-tuned `Hls.js` engine using Exponentially Weighted Moving Average (EWMA) sliding window bandwidth estimation.
- **Controls & Gestures:** Playback speed (0.5x–2.0x), dynamic resolution picker, Picture-in-Picture (PiP), theater mode, and mobile double-tap seek ripples ($\pm 10\text{s}$).
- **Live Diagnostics HUD:** Real-time buffer health monitoring, dropped frames percentage tracking with automated GPU decoder overload mitigation.
- **Theming & Sessions:** Cinema Dark (OLED) and Daylight Light modes with persistent token storage and system theme detection.

### 4. Cross-Platform App Shells
- **Progressive Web App (PWA):** Offline shell caching via Service Worker with standalone fullscreen mobile support.
- **Mobile (Android & iOS):** Capacitor native wrappers configured with deep linking, splash styling, and cleartext LAN streaming.
- **Desktop (Windows, macOS, Linux):** Lightweight Tauri wrapper providing native window chrome and hardware WebView acceleration.

### 5. Zero-Trust Security & Networking
- **HMAC Scoped Access:** Constant-time HMAC-SHA256 tokens scoping access to specific paths, actions, and expiration timeframes.
- **P2P Mesh & Tunnels:** WireGuard point-to-point mesh configuration and Cloudflare Tunnel / Rathole Noise reverse proxy blueprints.

---

## Quickstart Guide

### Prerequisites
- [Rust toolchain](https://rustup.rs/) (1.78+)
- [Node.js](https://nodejs.org/) (v18+) and `npm`
- [FFmpeg](https://ffmpeg.org/) (optional for CPU software playback, recommended for hardware transcoding)

### 1. Build Client Distribution
```powershell
# Install client dependencies
npm --prefix client install

# Compile TypeScript and Vite production bundle
npm --prefix client run build
```

### 2. Build & Launch Server Daemon
```powershell
# Compile optimized release binary (LTO + stripped)
cargo build --release

# Run daemon (serves both API and Web UI on port 8080)
.\target\release\aetherstream-server.exe --port 8080
```

### 3. Open in Browser
Navigate to **`http://localhost:8080`** to access the AetherStream universal interface.

---

## CLI Options

```
Usage: aetherstream-server.exe [OPTIONS]

Options:
  -h, --host <HOST>                Binding host [default: 0.0.0.0]
  -p, --port <PORT>                Listening port [default: 8080]
      --config-file <CONFIG_FILE>  Path to mount config [default: config/mounts.json]
  -h, --help                       Print help
  -V, --version                    Print version
```

---

## Core REST API Reference

| Method | Endpoint | Description |
|---|---|---|
| `GET` | `/api/v1/health` | Service health, hardware acceleration status, and active mounts |
| `GET` | `/api/v1/mounts` | List all registered virtual mounts and permissions |
| `GET` | `/api/v1/system/drives` | Enumerate host physical volumes and logical partitions |
| `GET` | `/api/v1/browse/:mount/*path` | Secure directory listing with file sizes and MIME types |
| `GET` | `/api/v1/stream/:mount/*path` | RFC 7233 zero-copy range request stream / direct download |
| `GET` | `/api/v1/transcode/hls/:mount/*path` | Dynamic RFC 8216 HLS master playlist for ABR playback |
| `POST` | `/api/v1/share/create` | Generate time-bounded scoped HMAC-SHA256 share tokens |
| `POST` | `/api/v1/upload/:mount/init` | Initialize multi-chunk resumable upload session |
| `POST` | `/api/v1/upload/:mount/chunk` | Upload 2 MiB chunk slice with SHA-256 verification |
| `POST` | `/api/v1/upload/:mount/complete` | Finalize upload and verify total file integrity |

---

## Verification & Test Harnesses

AetherStream maintains strict automated stage verification scripts located in `scripts/stages/`:

```powershell
# Run security test suite
cargo test --test security_tests

# Run individual stage validation audits
powershell -ExecutionPolicy Bypass -File .\scripts\stages\stage-1.ps1 -ValidateOnly
powershell -ExecutionPolicy Bypass -File .\scripts\stages\stage-2.ps1 -ValidateOnly
powershell -ExecutionPolicy Bypass -File .\scripts\stages\stage-3.ps1 -ValidateOnly
powershell -ExecutionPolicy Bypass -File .\scripts\stages\stage-4.ps1 -ValidateOnly
powershell -ExecutionPolicy Bypass -File .\scripts\stages\stage-5.ps1 -ValidateOnly
powershell -ExecutionPolicy Bypass -File .\scripts\stages\stage-6.ps1 -ValidateOnly
```

---

## Governance & Ledgers

All changes, task completions, and state transitions follow immutable append-only ledgers:
- [`CHANGELOG.md`](CHANGELOG.md): Cryptographically verified chronological event ledger.
- [`TODO.md`](TODO.md): Comprehensive task execution ledger.
- [`state.tracker.json`](state.tracker.json): Machine-readable state machine tracking all 6 stages, 18 phases, and 45 subphases.

---

## License

MIT OR Apache-2.0 © AetherStream Core Contributors

