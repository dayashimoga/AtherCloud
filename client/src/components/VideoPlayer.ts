import Hls from "hls.js";

export class VideoPlayerComponent {
  private videoElement: HTMLVideoElement;
  private hls: Hls | null = null;
  private hudRendition: HTMLElement;
  private hudBandwidth: HTMLElement;
  private hudBuffer: HTMLElement;
  private hudDropped: HTMLElement;
  private telemetryTimer: number | null = null;

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
      this.hls = new Hls({
        capLevelToPlayerSize: true,
        maxBufferLength: 30,
        maxMaxBufferLength: 60,
        lowLatencyMode: true,
      });

      this.hls.loadSource(url);
      this.hls.attachMedia(this.videoElement);

      this.hls.on(Hls.Events.LEVEL_SWITCHED, (_, data) => {
        const level = this.hls?.levels[data.level];
        if (level) {
          this.hudRendition.textContent = `${level.height}p (${Math.round(level.bitrate / 1000)}k)`;
        }
      });

      this.hls.on(Hls.Events.FRAG_LOADED, (_, data) => {
        const bw = Math.round(data.frag.stats.bandwidth / 1000);
        this.hudBandwidth.textContent = `${bw} kbps`;
      });
    } else {
      // Direct stream fallback
      this.videoElement.src = url;
      this.hudRendition.textContent = "Direct (Zero-Copy)";
    }

    this.videoElement.play().catch(() => {
      // Autoplay blocked by browser policy
    });

    this.startTelemetryLoop();
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

        // Emergency buffer starvation prevention
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
