# AGENT SKILLS & CAPABILITIES CATALOG
**System:** AetherStream Engineering Operations  
**Scope:** AI Subagents, Core Contributors, Automated CI Runners  
**Revision:** 2.0.0-PROD  

---

## 1. AGENT ARCHITECTURE & OPERATIONAL SKILLS

### 1.1 `Skill: HostEnvironmentAuditor`
- **Domain:** Stage 1 (Workspace Governance & Host Validation)
- **Capabilities:**
  - Evaluates PowerShell runtime versions (PowerShell 7+ / 5.1 fallback), execution policies (`RemoteSigned`, `Bypass`), and admin elevation (`[Security.Principal.WindowsPrincipal]`).
  - Probes hardware topologies: physical/logical CPU core counts, available RAM ceiling thresholds, and silicon video encoding ASIC blocks (Intel QSV via `/dev/dri` or MediaSDK, NVIDIA NVENC via CUDA driver API, Apple VideoToolbox via CoreMedia framework, Linux VA-API).
  - Inspects CLI toolchains (`git`, `gh`, `cargo`/`rustc`, `go`, `node`, `npm`, `ffmpeg`, `ffprobe`) for semver compatibility.
  - Provides auto-provisioning wrapper scripts or deterministic terminal mitigation recipes.

### 1.2 `Skill: AppendOnlyGovernanceGuard`
- **Domain:** Global Repository Contracts & Audit Ledgers
- **Capabilities:**
  - Implements transactional atomic appends (`Add-Content`) with pre- and post-write byte length assertions.
  - Computes SHA-256 cryptographic checkpoints before mutating `state.tracker.json`, `CHANGELOG.md`, and `TODO.md`.
  - Reverts immediately to backup snapshot if an operation triggers any line truncation or retro-modification.
  - Maintains Git pre-commit hooks enforcing zero-deletion rules on changelogs.

### 1.3 `Skill: StorageVirtualizationArchitect`
- **Domain:** Stage 2 (Storage Virtualization, I/O Subsystem & Granular RBAC)
- **Capabilities:**
  - Discovers cross-platform mount points: Windows drive letters (`C:\`, `D:\`, `E:\`, network UNC paths), POSIX mount points (`/mnt/*`, `/media/*`, `/Volumes/*`).
  - Implements SQLite WAL-mode virtual mount tables with granular permissions (`READ`, `WRITE`, `STREAM_ONLY`).
  - Enforces canonical path sandboxing (`dunce::canonicalize`, `std::path::Component` inspection) preventing directory traversal (`../`, encoded `%2e%2e`, null bytes, symlink breakout).
  - Implements RFC 7233 HTTP 206 Partial Content range requests, zero-copy socket transfers (`tokio_util::io::ReaderStream`), and chunked file upload handlers with SHA-256 chunk verification.
  - Generates scoped HMAC-SHA256 tokens and powers real-time filesystem indexing.

### 1.4 `Skill: AdaptiveMediaPipelineEngineer`
- **Domain:** Stage 3 (Hardware-Accelerated ABR Video Pipeline)
- **Capabilities:**
  - Invokes `ffprobe` to extract codec formats (H.264, HEVC, AV1, VP9), audio layouts, bitrates, GOP intervals.
  - Configures dynamic hardware acceleration encoder ladders (NVENC, QSV, VA-API, VideoToolbox, `libx264` software fallback).
  - Generates RFC 8216 HLS Master manifests (`master.m3u8`) and multi-rendition variants (1080p, 720p, 480p, 360p).
  - Implements JIT 2-4s segmenting into rolling LRU ring buffers with 5.1-to-stereo audio downmixing.
  - Runs automated disk space supervisors with LRU cache eviction and stress testing harnesses.

### 1.5 `Skill: UniversalClientEngineer`
- **Domain:** Stage 4 (Universal Client UI, Adaptive Video Player & App Shells)
- **Capabilities:**
  - Develops responsive, lightweight single-page applications (Vite + TypeScript + CSS custom properties).
  - Integrates Hls.js with custom EWMA bandwidth estimation and emergency buffer starvation avoidance.
  - Builds real-time telemetry HUDs tracking throughput (kbps), buffer health (seconds), and dropped frame counts.
  - Configures PWA service workers for offline caching, Capacitor mobile shells (Android/iOS), and Tauri desktop shells.

### 1.6 `Skill: IngressNetworkingSpecialist`
- **Domain:** Stage 5 (Zero-Config Secure Networking & Ingress Traversal)
- **Capabilities:**
  - Configures automated WireGuard/Tailscale mesh VPNs, Cloudflare Quick & Named Tunnels, and Rathole Noise-encrypted relays.
  - Implements intelligent routing: local LAN IP direct connect on same subnet, seamless fallback to tunnel when remote.
  - Enforces TLS 1.3, rate-limiting (leaky bucket), and penetration testing defenses.

### 1.7 `Skill: CIAutomationReleaseOrchestrator`
- **Domain:** Stage 6 (CI/CD, Cross-Compilation & Release Distribution)
- **Capabilities:**
  - Configures GitHub Actions multi-OS matrices (Linux x86_64/arm64, Windows x64, macOS Apple Silicon).
  - Bundles FFmpeg binary dependencies, strips symbols, and packages standalone zip/tarballs with SHA256 checksums.
  - Builds Android APK/AAB and iOS IPA artifacts via automated CI runners.
  - Seals releases with immutable changelog updates and end-to-end smoke tests.
