# SYSTEM ARCHITECTURE SPECIFICATION
**System:** AetherStream (Distributed Media Streaming & Virtual Storage Architecture)  
**Classification:** Production High-Throughput Specification  
**Architecture Revision:** 1.0.0  

---

## 1. END-TO-END ARCHITECTURAL OVERVIEW

AetherStream decouples high-performance asynchronous storage I/O, on-demand hardware-accelerated video transformation, zero-config secure edge tunneling, and an adaptive universal client platform.

```mermaid
flowchart TD
    subgraph ClientLayer ["Universal Multi-Platform Client Layer"]
        PWA["Responsive PWA (Vite + TypeScript + Hls.js)"]
        Mobile["Native Mobile Shells (Capacitor: Android / iOS)"]
        Desktop["Desktop Shells (Tauri: Win / Mac / Linux)"]
    end

    subgraph IngressLayer ["Secure Ingress & NAT Traversal"]
        CF["Cloudflare Tunnel (cloudflared quick-tunnel / named)"]
        RH["Rathole Zero-Overhead TCP/UDP Tunnel"]
        WG["WireGuard Mesh VPN Interface"]
    end

    subgraph CoreDaemon ["AetherStream Core Daemon (Rust / Axum Engine)"]
        Router["Axum Async Router & CORS"]
        HMAC["HMAC-SHA256 Token Auth & Rate Limiter"]
        MountMgr["Virtual Mount Registry & Path Traversal Guard"]
        StreamIO["Zero-Copy Chunked I/O & HTTP 206 Range Engine"]
        TranscodeCoord["JIT Transcode Orchestrator & Shm Buffer"]
    end

    subgraph StorageLayer ["Virtual Storage Subsystem"]
        Disk1["NVMe / SSD Primary Mount (/media/fast)"]
        Disk2["HDD Array Archive Mount (/media/storage)"]
        Removable["External Drive Mount (D:\ or /mnt/ext)"]
    end

    subgraph PipelineLayer ["Adaptive Video Transformation Engine"]
        Probe["Hardware Probe (NVENC / QSV / VAAPI / CPU)"]
        Segmenter["FFmpeg Pipe & HLS Multi-Rendition Segmenter"]
        MasterHLS["Dynamic Master Manifest (.m3u8) Generator"]
        CleanWorker["Segment Lifecycle Worker (TTL ring buffer)"]
    end

    PWA --> IngressLayer
    Mobile --> IngressLayer
    Desktop --> IngressLayer

    CF --> Router
    RH --> Router
    WG --> Router

    Router --> HMAC
    HMAC --> MountMgr
    MountMgr --> StreamIO
    MountMgr --> TranscodeCoord

    StreamIO --> StorageLayer
    TranscodeCoord --> Probe
    Probe --> Segmenter
    Segmenter --> CleanWorker
    CleanWorker --> MasterHLS
    MasterHLS --> Router
```

---

## 2. HOST ENGINE & STORAGE SUBSYSTEM (RUST + AXUM)

### 2.1 Concurrency Model & Async Reactor
The daemon runs on top of `tokio` multi-threaded runtime with asynchronous worker pools:
- **I/O Reactor:** Uses OS epoll / kqueue / IOCP to handle thousands of concurrent client connections with minimal context-switch overhead.
- **Worker Affinity:** File reads are scheduled onto `tokio::task::spawn_blocking` pools to prevent blocking reactor event loops on synchronous disk subsystem bottlenecks.
- **Zero-Copy Streaming:** Direct streaming utilizes `tokio_util::io::ReaderStream` wrapping async OS file handles, bypassing intermediate heap allocations via chunked transfer framing (`axum::body::Body`).

### 2.2 Path Sanitization & Virtual Mount Registry
Every virtual mount maps an external label (e.g. `movies`, `music`, `vault`) to a physical OS path.
```mermaid
sequenceDiagram
    participant C as Client
    participant R as Router / Security
    participant M as Mount Registry
    participant FS as Host FileSystem

    C->>R: GET /api/v1/stream/movies/sci-fi/dune.mp4
    R->>R: Decode URL, parse mount "movies"
    R->>M: Lookup base directory for "movies"
    M-->>R: Returns Physical Path: /mnt/storage/media/movies
    R->>R: Canonicalize requested target
    R->>R: Assert CanonicalPath.starts_with(BaseCanonicalPath)
    alt Path Escapes Mount Root (e.g. "../../../etc/passwd")
        R-->>C: 403 Forbidden (Path Traversal Detected)
    else Path Valid & Exists
        R->>FS: Open async file descriptor
        FS-->>R: File handle (size, metadata)
        R-->>C: 206 Partial Content / 200 OK (Stream)
    end
```

### 2.3 HTTP 206 Range Request Engine
- Supports standard RFC 7233 range headers: `Range: bytes=start-end`.
- Handles single-range requests with precise byte-level slicing:
  $$\text{Content-Range: bytes } s - e / \text{total}$$
- Automatically defaults to initial burst chunks ($2\text{ MiB}$) if the client omits the trailing range parameter (`Range: bytes=0-`), optimizing instant playback start while preventing unbounded disk read queues.

---

## 3. ADAPTIVE BITRATE TRANSCODE PIPELINE (FFMPEG JIT)

### 3.1 Hardware Acceleration Detection & Probe Matrix
On server boot and during runtime transcoding initialization, the engine executes a rapid capability probe:

```mermaid
flowchart LR
    Start["Probe Start"] --> CheckNVENC{"NVENC Available?"}
    CheckNVENC -- Yes --> UseNVENC["Encoder: h264_nvenc / hevc_nvenc<br>Accel: cuda"]
    CheckNVENC -- No --> CheckQSV{"Intel QSV Available?"}
    CheckQSV -- Yes --> UseQSV["Encoder: h264_qsv<br>Accel: qsv"]
    CheckQSV -- No --> CheckVAAPI{"VA-API Available?"}
    CheckVAAPI -- Yes --> UseVAAPI["Encoder: h264_vaapi<br>Accel: vaapi"]
    CheckVAAPI -- No --> CheckVT{"Apple VideoToolbox?"}
    CheckVT -- Yes --> UseVT["Encoder: h264_videotoolbox<br>Accel: videotoolbox"]
    CheckVT -- No --> Software["CPU Fallback: libx264<br>Preset: veryfast / ultrafast"]
```

### 3.2 JIT HLS Segmenter Architecture
1. **Source Discovery:** Media probe analyzes video/audio codecs, resolution, bitrate, and keyframe intervals using `ffprobe` JSON stream metadata.
2. **Dynamic Subprocess Pipe:** If the source codec is already browser-compatible (`H.264` + `AAC`) and resolution matches client capabilities, the engine engages **Direct Stream Mode** (zero transcoding CPU overhead).
3. **On-Demand Transcoding Ladder:** If adaptation is requested, FFmpeg is spawned as an async child process outputting segment slices (`.ts`) and variant manifests to a fast in-memory or ephemeral scratch ring buffer:
   - Fragment Duration: $4.0\text{ seconds}$
   - Keyframe interval (`GOP`): $2.0 \times \text{FPS}$ (e.g., GOP=60 for 30fps)
   - Master playlist (`master.m3u8`) links all profiles:
     ```m3u8
     #EXTM3U
     #EXT-X-VERSION:6
     #EXT-X-STREAM-INF:BANDWIDTH=4500000,RESOLUTION=1920x1080,CODECS="avc1.640028,mp4a.40.2"
     1080p/index.m3u8
     #EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1280x720,CODECS="avc1.4d401f,mp4a.40.2"
     720p/index.m3u8
     #EXT-X-STREAM-INF:BANDWIDTH=1200000,RESOLUTION=854x480,CODECS="avc1.4d401e,mp4a.40.2"
     480p/index.m3u8
     #EXT-X-STREAM-INF:BANDWIDTH=600000,RESOLUTION=640x360,CODECS="avc1.42e01e,mp4a.40.2"
     360p/index.m3u8
     ```
4. **Segment Cleanup Worker:** Background Tokio task evicts segments older than $60\text{ seconds}$ once elapsed playback horizons move forward, capping disk consumption per active stream to $< 50\text{ MB}$.

---

## 4. CLIENT-SIDE BUFFER & TELEMETRY ENGINE (HLS.JS + PWA)

### 4.1 Adaptive Bitrate Controller
The client integrates custom ABR algorithms with `hls.js`:
- **Bandwidth Moving Average:** Exponential Weighted Moving Average (EWMA) calculated over the last 5 downloaded fragments.
- **Buffer Starvation Avoidance:** If forward buffer falls below $6.0\text{ seconds}$, switch instantly to lower rendition without waiting for standard playlist switch heuristics.
- **Max Buffer Ahead:** Configured to $30.0\text{ seconds}$ to conserve memory on mobile devices and reduce unconsumed stream waste.

```mermaid
graph TD
    FragmentDownload["Fragment Download Complete"] --> CalcSpeed["Compute Throughput (Bytes / Elapsed Time)"]
    CalcSpeed --> UpdateEWMA["Update Bandwidth EWMA"]
    UpdateEWMA --> CheckBuffer{"Forward Buffer Health"}
    CheckBuffer -- "< 6s" --> EmergencyDrop["Force Immediate Drop to 360p/480p"]
    CheckBuffer -- "6s - 15s" --> MaintainOrStep["Maintain Current Rendition"]
    CheckBuffer -- "> 15s" --> UpgradeIfAllowed["Upgrade Rendition if Bandwidth Margin > 25%"]
    EmergencyDrop --> Telemetry["Emit Telemetry Event to UI HUD"]
    MaintainOrStep --> Telemetry
    UpgradeIfAllowed --> Telemetry
```

---

## 5. REMOTE CONNECTIVITY & ZERO-CONFIG NAT TRAVERSAL

Three complementary egress layers ensure access without public IPv4 or port forwarding:
1. **Cloudflare Tunnel (`cloudflared`):**
   - Direct encrypted outbound HTTPS tunnel to Cloudflare Edge.
   - Built-in TLS termination and HTTP/2 multiplexing.
   - Zero open router ports, resistant to ISP CGNAT.
2. **Rathole Tunnel:**
   - Lightweight zero-overhead Rust-based tunnel (`rathole`) for VPS egress.
   - Supports multiplexed TCP/UDP with Noise protocol encryption.
3. **Embedded WireGuard:**
   - Peer-to-peer authenticated VPN mesh for low-latency direct connectivity.

---

## 6. CROSS-PLATFORM BUILD STRATEGY

```mermaid
flowchart TD
    Repo["AetherStream Monorepo"]
    
    subgraph CoreBuild ["Backend Builds"]
        CargoBuild["Cargo Cross / Matrix Build"]
        CargoBuild --> Lin64["Linux x86_64 binary"]
        CargoBuild --> LinArm["Linux aarch64 binary"]
        CargoBuild --> Win64["Windows x64 executable"]
        CargoBuild --> MacArm["macOS Apple Silicon binary"]
    end

    subgraph ClientBuild ["Universal Client Packaging"]
        ViteBuild["Vite Static Production Build"]
        ViteBuild --> WebPWA["Installable PWA (IndexedDB offline store)"]
        ViteBuild --> CapSync["Capacitor Native Sync"]
        CapSync --> Android["Android Studio / Gradle (APK & AAB)"]
        CapSync --> iOS["Xcode Build (Unsigned IPA / xcarchive)"]
        ViteBuild --> TauriBuild["Tauri Desktop Engine"]
        TauriBuild --> DesktopBins["Win .msi / Linux .AppImage / Mac .dmg"]
    end

    Repo --> CoreBuild
    Repo --> ClientBuild
```

---

## 7. DATA ENCRYPTION & TOKEN AUTHORIZATION SPECIFICATION

### 7.1 Scoped HMAC Token Format
Access to media files and dynamic transcode streams is validated by high-speed HMAC-SHA256 signatures:
$$\text{Token} = \text{Base64URL}(\text{MountID} \mathbin{\Vert} \text{RelativePath} \mathbin{\Vert} \text{ExpiryTimestamp} \mathbin{\Vert} \text{Permissions} \mathbin{\Vert} \text{HMAC}(\text{Secret}, \dots))$$

- **Time-To-Live (TTL):** Configurable from 15 minutes (ephemeral share links) to 30 days (authenticated device sessions).
- **Scope Restriction:** Tokens are strictly bound to a designated mount and subpath. A token issued for `/movies/comedy` cannot access `/movies/action` or any other virtual mount.
- **Revocation List:** In-memory Bloom filter + SQLite persistence for instantaneous revocation of compromised share links.
