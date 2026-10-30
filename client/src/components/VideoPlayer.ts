import Hls from "hls.js";

export interface RenditionInfo {
  index: number;
  height: number;
  bitrate: number;
  name: string;
}

export class VideoPlayerComponent {
  private videoElement: HTMLVideoElement;
  private videoWrapper: HTMLElement | null = null;
  private videoContainerEl: HTMLElement | null = null;
  private hls: Hls | null = null;

  // Live Telemetry HUD Elements
  private hudRendition: HTMLElement;
  private hudBandwidth: HTMLElement;
  private hudBuffer: HTMLElement;
  private hudDropped: HTMLElement;
  private telemetryTimer: number | null = null;

  // Custom Controls UI Elements
  private controlsBar: HTMLElement | null = null;
  private playBtn: HTMLButtonElement | null = null;
  private rewindBtn: HTMLButtonElement | null = null;
  private forwardBtn: HTMLButtonElement | null = null;
  private muteBtn: HTMLButtonElement | null = null;
  private volumeSlider: HTMLInputElement | null = null;
  private seekRange: HTMLInputElement | null = null;
  private currentTimeEl: HTMLElement | null = null;
  private durationEl: HTMLElement | null = null;
  private speedSelect: HTMLSelectElement | null = null;
  private resolutionSelect: HTMLSelectElement | null = null;
  private pipBtn: HTMLButtonElement | null = null;
  private theaterBtn: HTMLButtonElement | null = null;
  private fullscreenBtn: HTMLButtonElement | null = null;

  // Double-tap gestures & ripple feedback
  private seekRippleLeft: HTMLElement | null = null;
  private seekRippleRight: HTMLElement | null = null;
  private lastTapTime = 0;
  private controlsHideTimer: number | null = null;
  private isSeeking = false;

  public onLevelsLoaded: ((levels: RenditionInfo[]) => void) | null = null;
  public onLevelSwitched: ((level: RenditionInfo | null) => void) | null = null;

  constructor() {
    this.videoElement = document.getElementById("video-element") as HTMLVideoElement;
    this.videoWrapper = document.getElementById("video-wrapper");
    this.videoContainerEl = document.querySelector(".video-container");

    this.hudRendition = document.getElementById("hud-rendition")!;
    this.hudBandwidth = document.getElementById("hud-bandwidth")!;
    this.hudBuffer = document.getElementById("hud-buffer")!;
    this.hudDropped = document.getElementById("hud-dropped")!;

    this.controlsBar = document.getElementById("player-controls");
    this.playBtn = document.getElementById("player-play-btn") as HTMLButtonElement | null;
    this.rewindBtn = document.getElementById("player-rewind-btn") as HTMLButtonElement | null;
    this.forwardBtn = document.getElementById("player-forward-btn") as HTMLButtonElement | null;
    this.muteBtn = document.getElementById("player-mute-btn") as HTMLButtonElement | null;
    this.volumeSlider = document.getElementById("player-volume-range") as HTMLInputElement | null;
    this.seekRange = document.getElementById("player-seek-range") as HTMLInputElement | null;
    this.currentTimeEl = document.getElementById("player-current-time");
    this.durationEl = document.getElementById("player-duration");
    this.speedSelect = document.getElementById("player-speed-select") as HTMLSelectElement | null;
    this.resolutionSelect = document.getElementById("player-resolution-select") as HTMLSelectElement | null;
    this.pipBtn = document.getElementById("player-pip-btn") as HTMLButtonElement | null;
    this.theaterBtn = document.getElementById("player-theater-btn") as HTMLButtonElement | null;
    this.fullscreenBtn = document.getElementById("player-fullscreen-btn") as HTMLButtonElement | null;

    this.seekRippleLeft = document.getElementById("seek-ripple-left");
    this.seekRippleRight = document.getElementById("seek-ripple-right");

    this.bindControls();
  }

  private bindControls() {
    // 1. Play / Pause
    if (this.playBtn) {
      this.playBtn.onclick = () => this.togglePlay();
    }
    this.videoElement.onclick = () => this.togglePlay();

    this.videoElement.onplay = () => {
      if (this.playBtn) this.playBtn.innerHTML = "&#10074;&#10074;";
      this.resetControlsTimeout();
    };

    this.videoElement.onpause = () => {
      if (this.playBtn) this.playBtn.innerHTML = "&#9654;";
      this.showControls();
    };

    // 2. Fast forward / Rewind 10s
    if (this.rewindBtn) {
      this.rewindBtn.onclick = () => this.seekRelative(-10);
    }
    if (this.forwardBtn) {
      this.forwardBtn.onclick = () => this.seekRelative(10);
    }

    // 3. Time tracking & Timeline scrubber
    this.videoElement.ontimeupdate = () => {
      if (!this.isSeeking && this.seekRange && this.videoElement.duration) {
        const pct = (this.videoElement.currentTime / this.videoElement.duration) * 100;
        this.seekRange.value = pct.toFixed(1);
      }
      if (this.currentTimeEl) {
        this.currentTimeEl.textContent = this.formatTime(this.videoElement.currentTime);
      }
    };

    this.videoElement.onloadedmetadata = () => {
      if (this.durationEl) {
        this.durationEl.textContent = this.formatTime(this.videoElement.duration);
      }
      if (this.seekRange) {
        this.seekRange.value = "0";
      }
    };

    if (this.seekRange) {
      this.seekRange.oninput = () => {
        this.isSeeking = true;
        if (this.videoElement.duration) {
          const targetTime = (parseFloat(this.seekRange!.value) / 100) * this.videoElement.duration;
          if (this.currentTimeEl) {
            this.currentTimeEl.textContent = this.formatTime(targetTime);
          }
        }
      };

      this.seekRange.onchange = () => {
        if (this.videoElement.duration) {
          this.videoElement.currentTime =
            (parseFloat(this.seekRange!.value) / 100) * this.videoElement.duration;
        }
        this.isSeeking = false;
      };
    }

    // 4. Volume & Mute
    if (this.volumeSlider) {
      this.volumeSlider.oninput = () => {
        const val = parseFloat(this.volumeSlider!.value);
        this.videoElement.volume = val;
        this.videoElement.muted = val === 0;
        this.updateMuteIcon();
      };
    }

    if (this.muteBtn) {
      this.muteBtn.onclick = () => {
        this.videoElement.muted = !this.videoElement.muted;
        this.updateMuteIcon();
      };
    }

    // 5. Playback Speed Selector
    if (this.speedSelect) {
      this.speedSelect.onchange = () => {
        this.videoElement.playbackRate = parseFloat(this.speedSelect!.value);
      };
    }

    // 6. Resolution Ladder Selection
    if (this.resolutionSelect) {
      this.resolutionSelect.onchange = () => {
        const levelIdx = parseInt(this.resolutionSelect!.value, 10);
        this.setLevel(levelIdx);
      };
    }

    // 7. Picture-in-Picture
    if (this.pipBtn) {
      this.pipBtn.onclick = async () => {
        try {
          if (document.pictureInPictureElement) {
            await document.exitPictureInPicture();
          } else if (this.videoElement.requestPictureInPicture) {
            await this.videoElement.requestPictureInPicture();
          }
        } catch (err) {
          console.warn("Picture-in-picture failed:", err);
        }
      };
    }

    // 8. Theater Mode Toggle
    if (this.theaterBtn) {
      this.theaterBtn.onclick = () => {
        this.videoContainerEl?.classList.toggle("theater-mode");
      };
    }

    // 9. Fullscreen Toggle
    if (this.fullscreenBtn) {
      this.fullscreenBtn.onclick = () => {
        if (!document.fullscreenElement) {
          this.videoWrapper?.requestFullscreen?.();
        } else {
          document.exitFullscreen?.();
        }
      };
    }

    // 10. Mobile Touch Double-Tap Gestures & Inactivity Timer
    if (this.videoWrapper) {
      this.videoWrapper.onmousemove = () => this.handleUserActivity();
      this.videoWrapper.ontouchstart = (e) => this.handleTouch(e);
    }

    // 11. Keyboard Navigation
    window.addEventListener("keydown", (e) => {
      // Only handle if video modal is visible and no input is focused
      if (
        document.activeElement?.tagName === "INPUT" ||
        document.activeElement?.tagName === "SELECT" ||
        document.getElementById("player-modal")?.classList.contains("hidden")
      ) {
        return;
      }

      switch (e.key) {
        case " ":
          e.preventDefault();
          this.togglePlay();
          break;
        case "ArrowLeft":
          e.preventDefault();
          this.seekRelative(-10);
          break;
        case "ArrowRight":
          e.preventDefault();
          this.seekRelative(10);
          break;
        case "f":
        case "F":
          this.fullscreenBtn?.click();
          break;
        case "t":
        case "T":
          this.theaterBtn?.click();
          break;
        case "m":
        case "M":
          this.muteBtn?.click();
          break;
      }
    });
  }

  private togglePlay() {
    if (this.videoElement.paused) {
      this.videoElement.play();
    } else {
      this.videoElement.pause();
    }
  }

  private seekRelative(seconds: number) {
    this.videoElement.currentTime = Math.max(
      0,
      Math.min(this.videoElement.currentTime + seconds, this.videoElement.duration || 0)
    );
    if (seconds < 0) {
      this.flashRipple(this.seekRippleLeft);
    } else {
      this.flashRipple(this.seekRippleRight);
    }
  }

  private flashRipple(el: HTMLElement | null) {
    if (!el) return;
    el.classList.remove("hidden");
    setTimeout(() => {
      el.classList.add("hidden");
    }, 450);
  }

  private handleTouch(e: TouchEvent) {
    if (e.touches.length !== 1) return;
    const now = Date.now();
    const touchX = e.touches[0].clientX;
    const rect = this.videoWrapper?.getBoundingClientRect();

    if (rect && now - this.lastTapTime < 300) {
      // Double tap detected
      const relX = touchX - rect.left;
      if (relX < rect.width * 0.35) {
        this.seekRelative(-10);
      } else if (relX > rect.width * 0.65) {
        this.seekRelative(10);
      } else {
        this.togglePlay();
      }
      this.lastTapTime = 0;
    } else {
      // Single tap -> toggle controls
      this.lastTapTime = now;
      this.toggleControls();
    }
  }

  private handleUserActivity() {
    this.showControls();
    this.resetControlsTimeout();
  }

  private showControls() {
    this.controlsBar?.classList.remove("inactive");
  }

  private hideControls() {
    if (!this.videoElement.paused) {
      this.controlsBar?.classList.add("inactive");
    }
  }

  private toggleControls() {
    if (this.controlsBar?.classList.contains("inactive")) {
      this.showControls();
      this.resetControlsTimeout();
    } else {
      this.hideControls();
    }
  }

  private resetControlsTimeout() {
    if (this.controlsHideTimer !== null) {
      clearTimeout(this.controlsHideTimer);
    }
    this.controlsHideTimer = window.setTimeout(() => {
      this.hideControls();
    }, 3000);
  }

  private updateMuteIcon() {
    if (!this.muteBtn) return;
    if (this.videoElement.muted || this.videoElement.volume === 0) {
      this.muteBtn.innerHTML = "&#128263;"; // Muted
    } else if (this.videoElement.volume < 0.5) {
      this.muteBtn.innerHTML = "&#128265;"; // Low
    } else {
      this.muteBtn.innerHTML = "&#128266;"; // High
    }
  }

  private formatTime(seconds: number): string {
    if (isNaN(seconds) || seconds < 0) return "00:00";
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = Math.floor(seconds % 60);

    const pad = (n: number) => n.toString().padStart(2, "0");
    if (h > 0) {
      return `${pad(h)}:${pad(m)}:${pad(s)}`;
    }
    return `${pad(m)}:${pad(s)}`;
  }

  loadSource(url: string, isHls = false) {
    this.destroy();

    // Reset controls UI state
    if (this.seekRange) this.seekRange.value = "0";
    if (this.currentTimeEl) this.currentTimeEl.textContent = "00:00";
    if (this.durationEl) this.durationEl.textContent = "00:00";
    if (this.speedSelect) this.speedSelect.value = "1.0";
    this.videoElement.playbackRate = 1.0;

    if (this.resolutionSelect) {
      this.resolutionSelect.innerHTML = "<option value='-1' selected>Auto ABR</option>";
    }

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
        this.populateResolutionPicker(levels);
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

  private populateResolutionPicker(levels: RenditionInfo[]) {
    if (!this.resolutionSelect) return;
    this.resolutionSelect.innerHTML = "<option value='-1' selected>Auto ABR</option>";
    levels.forEach((lvl) => {
      const opt = document.createElement("option");
      opt.value = lvl.index.toString();
      opt.textContent = lvl.name;
      this.resolutionSelect!.appendChild(opt);
    });
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
      if (this.resolutionSelect) {
        this.resolutionSelect.value = levelIndex.toString();
      }
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
    if (this.controlsHideTimer !== null) {
      clearTimeout(this.controlsHideTimer);
      this.controlsHideTimer = null;
    }
    if (this.hls) {
      this.hls.destroy();
      this.hls = null;
    }
    this.videoElement.pause();
    this.videoElement.removeAttribute("src");
    this.videoElement.load();
    this.videoContainerEl?.classList.remove("theater-mode");
  }
}
