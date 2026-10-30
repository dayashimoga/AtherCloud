import Hls from "hls.js";

export interface RenditionInfo {
  index: number;
  height: number;
  bitrate: number;
  name: string;
}

export class VideoPlayerComponent {
  private videoElement: HTMLVideoElement;
  private hls: Hls | null = null;
  private hudRendition: HTMLElement;
  private hudBandwidth: HTMLElement;
  private hudBuffer: HTMLElement;
  private hudDropped: HTMLElement;
  private telemetryTimer: number | null = null;

  public onLevelsLoaded: ((levels: RenditionInfo[]) => void) | null = null;
  public onLevelSwitched: ((level: RenditionInfo | null) => void) | null = null;

  constructor() {
    this.videoElement = document.getElementById("video-element") as HTMLVideoElement;
    this.hudRendition = document.getElementById("hud-rendition")!;
    this.hudBandwidth = document.getElementById("hud-bandwidth")!;
    this.hudBuffer = document.getElementById("hud-buffer")!;
    this.hudDropped = document.getElementById("hud-dropped")!;
  }

  loadSource(url: string, isHls = false) {
    this.destroy();

    if (isHls && Hls.isSupported()) {
      // Fine-tuned ABR tuning (EWMA sliding window, conservative margins & buffer horizons)
      this.hls = new Hls({
        capLevelToPlayerSize: true,
        maxBufferLength: 30, // 30s forward buffer horizon
        maxMaxBufferLength: 60, // 60s max buffer cap
        maxBufferSize: 60 * 1024 * 1024, // 60 MB buffer size ceiling
        maxBufferHole: 0.5,
        backBufferLength: 30, // 30s backward buffer preservation for instant seek back
        lowLatencyMode: true,

        // EWMA Sliding Window Bandwidth Estimation & ABR Tuning
        abrEwmaFastLive: 3.0,
        abrEwmaSlowLive: 9.0,
        abrEwmaFastVoD: 3.0,
        abrEwmaSlowVoD: 9.0,
        abrEwmaDefaultEstimate: 5000000, // Initial 5 Mbps baseline to prevent starting at lowest rendition
        abrBandWidthFactor: 0.85, // 15% safety headroom
        abrBandWidthUpFactor: 0.70, // Requires 30% headroom before ascending ladder
        abrMaxWithRealBitrate: true,

        fragLoadingTimeOut: 20000,
        manifestLoadingTimeOut: 15000,
        levelLoadingTimeOut: 15000,
      });

      this.hls.loadSource(url);
      this.hls.attachMedia(this.videoElement);

      this.hls.on(Hls.Events.MANIFEST_PARSED, () => {
        const levels = this.getLevels();
        this.onLevelsLoaded?.(levels);
      });

      this.hls.on(Hls.Events.LEVEL_SWITCHED, (_, data) => {
        const level = this.hls?.levels[data.level];
        if (level) {
          const info: RenditionInfo = {
            index: data.level,
            height: level.height,
            bitrate: level.bitrate,
            name: `${level.height}p (${Math.round(level.bitrate / 1000)}k)`,
          };
          this.hudRendition.textContent = info.name;
          this.onLevelSwitched?.(info);
        }
      });

      this.hls.on(Hls.Events.FRAG_LOADED, (_, data) => {
        const stats = data.frag.stats as any;
        const bw = Math.round((stats.bandwidth || this.hls?.bandwidthEstimate || 0) / 1000);
        this.hudBandwidth.textContent = `${bw} kbps`;
      });

      // Adaptive Error Recovery Strategy
      this.hls.on(Hls.Events.ERROR, (_, data) => {
        if (data.fatal) {
          switch (data.type) {
            case Hls.ErrorTypes.NETWORK_ERROR:
              console.warn("[HLS] Fatal network error encountered, attempting recovery...", data);
              this.hls?.startLoad();
              break;
            case Hls.ErrorTypes.MEDIA_ERROR:
              console.warn("[HLS] Fatal media error encountered, recovering media error...", data);
              this.hls?.recoverMediaError();
              break;
            default:
              console.error("[HLS] Unrecoverable fatal error encountered, falling back to direct stream:", data);
              this.destroy();
              this.loadSource(url, false);
              break;
          }
        }
      });
    } else if (isHls && this.videoElement.canPlayType("application/vnd.apple.mpegurl")) {
      // Native Apple HLS Fallback (Safari macOS / iOS WebKit)
      this.videoElement.src = url;
      this.hudRendition.textContent = "Native Apple HLS";
    } else {
      // Zero-Copy Direct Stream Fallback
      this.videoElement.src = url;
      this.hudRendition.textContent = "Direct (Zero-Copy)";
    }

    this.videoElement.play().catch(() => {
      // Autoplay blocked by browser policy until user gesture
    });

    this.startTelemetryLoop();
  }

  public getLevels(): RenditionInfo[] {
    if (!this.hls) return [];
    return this.hls.levels.map((lvl, idx) => ({
      index: idx,
      height: lvl.height,
      bitrate: lvl.bitrate,
      name: `${lvl.height}p (${Math.round(lvl.bitrate / 1000)}k)`,
    }));
  }

  public getCurrentLevel(): number {
    return this.hls ? this.hls.currentLevel : -1;
  }

  public setLevel(levelIndex: number) {
    if (this.hls) {
      this.hls.currentLevel = levelIndex;
      if (levelIndex === -1) {
        this.hudRendition.textContent = "Auto ABR";
        this.onLevelSwitched?.(null);
      } else {
        const lvl = this.hls.levels[levelIndex];
        if (lvl) {
          const info: RenditionInfo = {
            index: levelIndex,
            height: lvl.height,
            bitrate: lvl.bitrate,
            name: `${lvl.height}p (${Math.round(lvl.bitrate / 1000)}k)`,
          };
          this.hudRendition.textContent = info.name;
          this.onLevelSwitched?.(info);
        }
      }
    }
  }

  public getVideoElement(): HTMLVideoElement {
    return this.videoElement;
  }

  public isHlsActive(): boolean {
    return this.hls !== null;
  }

  private startTelemetryLoop() {
    this.stopTelemetryLoop();
    this.telemetryTimer = window.setInterval(() => {
      // 1. Buffer health calculation
      if (this.videoElement.buffered.length > 0) {
        const current = this.videoElement.currentTime;
        let forwardBuffer = 0;
        for (let i = 0; i < this.videoElement.buffered.length; i++) {
          if (
            this.videoElement.buffered.start(i) <= current &&
            current <= this.videoElement.buffered.end(i)
          ) {
            forwardBuffer = this.videoElement.buffered.end(i) - current;
            break;
          }
        }
        this.hudBuffer.textContent = `${forwardBuffer.toFixed(1)}s`;

        // Emergency buffer starvation prevention (<6s)
        if (forwardBuffer < 6.0 && this.hls && this.hls.currentLevel > 0) {
          console.warn("[TELEMETRY] Buffer starvation alert (<6s). Downgrading rendition ladder.");
          this.hls.nextLevel = 0; // Drop to lowest safe rendition
        }
      }

      // 2. Dropped frames calculation
      if ("getVideoPlaybackQuality" in this.videoElement) {
        const quality = this.videoElement.getVideoPlaybackQuality();
        this.hudDropped.textContent = `${quality.droppedVideoFrames}`;
      }
    }, 1000);
  }

  private stopTelemetryLoop() {
    if (this.telemetryTimer !== null) {
      clearInterval(this.telemetryTimer);
      this.telemetryTimer = null;
    }
  }

  destroy() {
    this.stopTelemetryLoop();
    if (this.hls) {
      this.hls.destroy();
      this.hls = null;
    }
    this.videoElement.pause();
    this.videoElement.removeAttribute("src");
    this.videoElement.load();
  }
}
