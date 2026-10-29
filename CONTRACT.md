# GLOBAL ENGINEERING CONTRACT & GOVERNANCE MANIFESTO
**Project:** AetherStream (Ultra-Low-Latency Personal Media Streaming & Zero-Copy Storage Ecosystem)  
**Standard:** Production-Grade High-Throughput Systems Engineering  
**Version:** 1.0.0-PROD  
**Effective Date:** 2026-10-29  

---

## 1. PREAMBLE & CORE TENETS
This contract constitutes the non-negotiable architectural, governance, and operational baseline for the **AetherStream** ecosystem. All automated agents, developers, and CI/CD pipelines interacting with this repository MUST comply with every clause articulated herein. Violations of this contract trigger immediate build rejection and state rollback.

### Tenet I: Deterministic State Progression
All mutations to workspace state, phase progression, and operational readiness MUST be driven through deterministic, immutable state transitions tracked in `state.tracker.json`. Unrecorded modifications are considered catastrophic regressions.

### Tenet II: Strict Append-Only Records
`CHANGELOG.md` and `TODO.md` are append-only audit ledgers. Modifying historical entries, truncating past items, reordering timestamps, or deleting historical records is strictly prohibited and guarded via automated hash checks.

### Tenet III: Zero-Regression & Zero-Compromise Security
Virtual mounts must never expose host root filesystems. All storage access enforces canonicalized path resolution, symlink escape neutralization, and HMAC token validation. Range request parsing must defend against denial-of-service (DoS) via overlapping or unbounded byte ranges.

### Tenet IV: Native Platform Abstraction
The backend server binary must compile natively across Linux (`x86_64`, `aarch64`), Windows (`x86_64`), and macOS (`Apple Silicon arm64`) without runtime emulation. Client delivery spans progressive web apps (PWA) and native mobile/desktop binaries (Capacitor/Tauri) with 100% functional parity.

---

## 2. SERVICE LEVEL AGREEMENTS (SLAs) & PERFORMANCE THRESHOLDS

| Metric Category | Target SLA | Hard Ceiling / Budget | Measurement Technique |
| :--- | :--- | :--- | :--- |
| **Time-To-First-Frame (TTFF)** | `< 350 ms` (Direct Stream)<br>`< 800 ms` (JIT Transcode) | `1200 ms` maximum | Client performance timer from manifest request to `Hls.Events.FRAG_LOADED` |
| **HTTP Range Request Latency** | `p95 < 15 ms`, `p99 < 35 ms` | `50 ms` under 1,000 req/sec | Axum middleware telemetry over loopback benchmark |
| **Memory Footprint (Idle)** | `< 25 MB` RSS (Rust Daemon) | `50 MB` RSS | OS process memory monitor (`Get-Process` / `/proc/$PID/statm`) |
| **Memory Footprint (Streaming)**| `< 75 MB` RSS per 10 streams | `150 MB` RSS | OS process memory monitor under active chunked reads |
| **CPU Utilization (Direct Read)**| `< 1.5%` per 4K direct stream | `3.0%` of a single CPU core | OS thread metrics |
| **Buffer Stall Rate** | `0 stalls` on stable connection | `< 0.2 stalls/hour` on fluctuating 4G | Client telemetry reporting `Hls.Events.BUFFER_STALLED` |
| **File I/O Throughput** | Multi-gigabit saturating link | Zero-copy kernel buffer transfer (`tokio::fs` + chunked framing) | Transfer benchmarks across local loopback |

---

## 3. APPEND-ONLY GOVERNANCE SPECIFICATION

### 3.1 Ledger Immutability Guard
1. **Change Ledger (`CHANGELOG.md`):**
   - Each transition MUST append a standardized block containing:
     - ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`).
     - Stage ID (`STAGE_X`), Phase ID (`PHASE_X.Y`), and Subphase ID (`SUBPHASE_X.Y.Z`).
     - List of modified, created, or deprecated components.
     - Cryptographic checksum / Git commit SHA anchor.
     - Qualitative architectural delta justification.
   - Any script performing write operations to `CHANGELOG.md` MUST verify `FileLength(New) >= FileLength(Old)` and assert that `OldContent` is an exact byte-for-byte prefix of `NewContent`.

2. **Task Ledger (`TODO.md`):**
   - No task may ever be deleted or removed.
   - States strictly transition monotonically: `PENDING` -> `IN_PROGRESS` -> `COMPLETED` (or `BLOCKED` with mandatory blocker rationale).
   - Updates are registered by appending a transition trace block referencing the original Task ID with execution timestamp, executor signature, and validation test receipt.

---

## 4. STORAGE VIRTUALIZATION & ACCESS SECURITY CONTRACT

### 4.1 Path Traversal Defenses
1. **Canonicalization:** Every user-supplied path string MUST be resolved against the registered root of the target mount point using canonical path expansion (`dunce::canonicalize` or `std::fs::canonicalize`).
2. **Mount Boundary Confinement:** The canonicalized absolute path MUST start with the canonicalized absolute path of the mount root:
   $$\text{Path}_{\text{canonical}}(\text{Request}) \subseteq \text{Path}_{\text{canonical}}(\text{MountRoot})$$
3. **Symlink Guarding:** Symlinks pointing outside the mount root MUST be rejected with `403 Forbidden` (`Err(AccessDenied::SymlinkEscapingBoundary)`).
4. **Reserved Character Screening:** Null bytes (`\0`), path normalization bypasses (`..`, `/.`, `\\`), and hidden filesystem control files (`.git`, `.env`, system files) are blocked at the router layer.

### 4.2 Range Request & I/O Protocol
1. Requests specifying `Range: bytes=start-end` MUST return `206 Partial Content` with `Content-Range: bytes start-end/total` and `Accept-Ranges: bytes`.
2. Multi-part ranges and overlapping ranges designed for algorithmic complexity attacks MUST be sanitized or restricted to single bounded slices.
3. Reading operations MUST utilize non-blocking asynchronous chunks (`AsyncReadExt`) with bounded buffer pools (default `64 KiB` - `2 MiB` sliding frames) to prevent memory ballooning.

---

## 5. HARDWARE-ACCELERATED TRANSCODING CONTRACT

### 5.1 Dynamic Probe & Profile Fallback Ladder
The transcoding daemon probe pipeline MUST execute in the following priority order:
1. **NVIDIA NVENC** (`h264_nvenc` / `hevc_nvenc`)
2. **Intel QuickSync Video** (`h264_qsv` / `hevc_qsv`)
3. **Linux VA-API** (`h264_vaapi`)
4. **macOS VideoToolbox** (`h264_videotoolbox`)
5. **Software Baseline** (`libx264` with preset `veryfast` and rate control `CRF 23`)

### 5.2 HLS Rendition Ladder Specifications

| Profile Tag | Resolution | Video Bitrate | Audio Bitrate | Segment Duration | Audio Codec |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `1080p_high` | $1920 \times 1080$ | $4,500\text{ kbps}$ | $192\text{ kbps}$ | 4.0 seconds | AAC-LC stereo |
| `720p_med` | $1280 \times 720$ | $2,500\text{ kbps}$ | $128\text{ kbps}$ | 4.0 seconds | AAC-LC stereo |
| `480p_low` | $854 \times 480$ | $1,200\text{ kbps}$ | $96\text{ kbps}$ | 4.0 seconds | AAC-LC stereo |
| `360p_min` | $640 \times 360$ | $600\text{ kbps}$ | $64\text{ kbps}$ | 4.0 seconds | AAC-LC stereo |

All playlists must generate compliant RFC 8216 HLS manifests (`#EXTM3U`, `#EXT-X-VERSION:6`, `#EXT-X-STREAM-INF`).

---

## 6. SCRIPT AUTOMATION & BOOTSTRAP PROTOCOL
1. All PowerShell scripts MUST begin with:
   ```powershell
   $ErrorActionPreference = "Stop"
   Set-StrictMode -Version Latest
   ```
2. Scripts must execute reliably on PowerShell 7+ on Linux, macOS, and Windows, as well as Windows PowerShell 5.1.
3. Every execution step MUST log standardized structured outputs:
   - `[INFO]`: Operational status.
   - `[SUCCESS]`: Successful validation assertion.
   - `[WARN]`: Non-critical configuration discrepancy.
   - `[ERROR]`: Terminal failure halting the pipeline.
4. Stage execution MUST be atomic: if a subphase validation fails, `state.tracker.json` remains untouched, preventing incomplete state flags.

---

## 7. RATIFICATION SIGNATURE
This document is canonically locked upon generation. Any modification requires an incremented version number, unanimous review approval, and an atomic change entry appended to `CHANGELOG.md`.

*Signed for the Ecosystem: Principal Systems & Software Architecture Committee*
