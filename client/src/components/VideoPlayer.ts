import Hls from "hls.js";
import { PlaybackSettingsManager } from "../services/playbackSettings";

export interface RenditionInfo {
  index: number;
  height: number;
  width?: number;
  bitrate: number;
  frameRate?: number;
  codecs?: string;
  name: string;
  shortName: string;
  label: string;
  isAuto?: boolean;
}

export class VideoPlayerComponent {
  private videoElement: HTMLVideoElement;
  private videoWrapper: HTMLElement | null = null;
  private videoContainerEl: HTMLElement | null = null;
  private hls: Hls | null = null;

  // Persistent Settings
  private settingsManager: PlaybackSettingsManager;

  // Live Telemetry HUD Elements
  private telemetryHudEl: HTMLElement | null = null;
  private hudRendition: HTMLElement;
  private hudBandwidth: HTMLElement;
  private hudBuffer: HTMLElement;
  private hudDropped: HTMLElement;
  private hudShifts: HTMLElement | null = null;
  private hudCodec: HTMLElement | null = null;
  private telemetryTimer: number | null = null;
  private abrShiftCount = 0;

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
  private qualityBadgeEl: HTMLElement | null = null;
  private qualityToastEl: HTMLElement | null = null;
  private toastTimer: number | null = null;
  private subtitlesBtn: HTMLButtonElement | null = null;
  private subtitlesSelect: HTMLSelectElement | null = null;
  private statsBtn: HTMLButtonElement | null = null;
  private pipBtn: HTMLButtonElement | null = null;
  private theaterBtn: HTMLButtonElement | null = null;
  private fullscreenBtn: HTMLButtonElement | null = null;

  // Double-tap gestures & ripple feedback
  private seekRippleLeft: HTMLElement | null = null;
  private seekRippleRight: HTMLElement | null = null;
  private lastTapTime = 0;
  private controlsHideTimer: number | null = null;
  private isSeeking = false;

  // Resolution and rendition state
  private isManualResolution = false;
  private currentActiveLevel: RenditionInfo | null = null;
  private availableLevels: RenditionInfo[] = [];

  public onLevelsLoaded: ((levels: RenditionInfo[]) => void) | null = null;
  public onLevelSwitched: ((level: RenditionInfo | null) => void) | null = null;

  constructor() {
    this.settingsManager = PlaybackSettingsManager.getInstance();

    this.videoElement = document.getElementById("video-element") as HTMLVideoElement;
    this.videoWrapper = document.getElementById("video-wrapper");
    this.videoContainerEl = document.querySelector(".video-container");

    this.telemetryHudEl = document.getElementById("telemetry-hud");
    this.hudRendition = document.getElementById("hud-rendition")!;
    this.hudBandwidth = document.getElementById("hud-bandwidth")!;
    this.hudBuffer = document.getElementById("hud-buffer")!;
    this.hudDropped = document.getElementById("hud-dropped")!;
    this.hudShifts = document.getElementById("hud-shifts");
    this.hudCodec = document.getElementById("hud-codec");

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
    this.qualityBadgeEl = document.getElementById("player-quality-badge");
    this.qualityToastEl = document.getElementById("player-quality-toast");
    this.subtitlesBtn = document.getElementById("player-subtitles-btn") as HTMLButtonElement | null;
    this.subtitlesSelect = document.getElementById("player-subtitles-select") as HTMLSelectElement | null;
    this.statsBtn = document.getElementById("player-stats-btn") as HTMLButtonElement | null;
    this.pipBtn = document.getElementById("player-pip-btn") as HTMLButtonElement | null;
    this.theaterBtn = document.getElementById("player-theater-btn") as HTMLButtonElement | null;
    this.fullscreenBtn = document.getElementById("player-fullscreen-btn") as HTMLButtonElement | null;

    this.seekRippleLeft = document.getElementById("seek-ripple-left");
    this.seekRippleRight = document.getElementById("seek-ripple-right");

    this.applyStoredSettings();
    this.bindControls();
  }

  /**
   * Restores persistent playback preferences from localStorage:
   * Volume level, Mute state, Playback rate, Subtitle preferences
   */
  public applyStoredSettings() {
    const s = this.settingsManager.getSettings();

    // Volume & Mute
    this.videoElement.volume = s.volume;
    this.videoElement.muted = s.muted;
    if (this.volumeSlider) {
      this.volumeSlider.value = s.volume.toString();
    }
    this.updateMuteIcon();

    // Playback Speed
    this.videoElement.playbackRate = s.playbackRate;
    if (this.speedSelect) {
      this.speedSelect.value = s.playbackRate.toString();
    }

    // Subtitles
    if (this.subtitlesSelect) {
      this.subtitlesSelect.value = s.subtitlesEnabled ? s.subtitleLanguage : "off";
    }
    this.updateSubtitlesButton(s.subtitlesEnabled);
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
      // Re-apply rate on metadata load in case browser resets
      this.videoElement.playbackRate = this.settingsManager.getPlaybackRate();
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

    // 4. Volume & Mute with persistent localStorage storage
    if (this.volumeSlider) {
      this.volumeSlider.oninput = () => {
        const val = parseFloat(this.volumeSlider!.value);
        this.videoElement.volume = val;
        this.videoElement.muted = val === 0;
        this.updateMuteIcon();
        this.settingsManager.setVolume(val);
      };
    }

    if (this.muteBtn) {
      this.muteBtn.onclick = () => {
        this.videoElement.muted = !this.videoElement.muted;
        this.updateMuteIcon();
        this.settingsManager.setMuted(this.videoElement.muted);
      };
    }

    // 5. Playback Speed Selector with persistent storage
    if (this.speedSelect) {
      this.speedSelect.onchange = () => {
        const rate = parseFloat(this.speedSelect!.value);
        this.videoElement.playbackRate = rate;
        this.settingsManager.setPlaybackRate(rate);
        this.showQualityToast(`Speed: ${rate}x`);
      };
    }

    // 6. Resolution Ladder Selection
    if (this.resolutionSelect) {
      this.resolutionSelect.onchange = () => {
        const levelIdx = parseInt(this.resolutionSelect!.value, 10);
        this.setLevel(levelIdx);
        this.settingsManager.setPreferredResolution(levelIdx);
      };
    }

    // 7. Subtitles & Closed Captions with persistent storage
    if (this.subtitlesSelect) {
      this.subtitlesSelect.onchange = () => {
        const lang = this.subtitlesSelect!.value;
        const enabled = lang !== "off";
        this.settingsManager.setSubtitlePreference(lang, enabled);
        this.applySubtitleTrack(lang, enabled);
        this.updateSubtitlesButton(enabled);
        this.showQualityToast(enabled ? `Subtitles: ${lang.toUpperCase()}` : "Subtitles: Off");
      };
    }

    if (this.subtitlesBtn) {
      this.subtitlesBtn.onclick = () => {
        const enabled = this.settingsManager.toggleSubtitles();
        const lang = this.settingsManager.getSubtitleLanguage();
        if (this.subtitlesSelect) {
          this.subtitlesSelect.value = enabled ? lang : "off";
        }
        this.applySubtitleTrack(lang, enabled);
        this.updateSubtitlesButton(enabled);
        this.showQualityToast(enabled ? `Subtitles: ${lang.toUpperCase()}` : "Subtitles: Off");
      };
    }

    // 8. Live Telemetry HUD Toggle Button
    if (this.statsBtn) {
      this.statsBtn.onclick = () => this.toggleTelemetryHud();
    }

    // 9. Picture-in-Picture
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

    // 10. Theater Mode Toggle
    if (this.theaterBtn) {
      this.theaterBtn.onclick = () => {
        this.videoContainerEl?.classList.toggle("theater-mode");
      };
    }

    // 11. Fullscreen Toggle
    if (this.fullscreenBtn) {
      this.fullscreenBtn.onclick = () => {
        if (!document.fullscreenElement) {
          this.videoWrapper?.requestFullscreen?.();
        } else {
          document.exitFullscreen?.();
        }
      };
    }

    // 12. Mobile Touch Double-Tap Gestures & Inactivity Timer
    if (this.videoWrapper) {
      this.videoWrapper.onmousemove = () => this.handleUserActivity();
      this.videoWrapper.ontouchstart = (e) => this.handleTouch(e);
    }

    // 13. Keyboard Navigation
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
        case "c":
        case "C":
          this.subtitlesBtn?.click();
          break;
        case "d":
        case "D":
          this.toggleTelemetryHud();
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

  public toggleTelemetryHud() {
    if (this.telemetryHudEl) {
      this.telemetryHudEl.classList.toggle("hidden");
    }
  }

  public togglePlay() {
    if (this.videoElement.paused) {
      this.videoElement.play();
    } else {
      this.videoElement.pause();
    }
  }

  public seekRelative(seconds: number) {
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

  private updateSubtitlesButton(enabled: boolean) {
    if (!this.subtitlesBtn) return;
    if (enabled) {
      this.subtitlesBtn.classList.add("active");
      this.subtitlesBtn.title = "Subtitles Active (C to toggle)";
    } else {
      this.subtitlesBtn.classList.remove("active");
      this.subtitlesBtn.title = "Subtitles Off (C to toggle)";
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
    this.abrShiftCount = 0;
    this.isManualResolution = false;
    this.currentActiveLevel = null;
    this.availableLevels = [];
    this.updateQualityBadge("AUTO", "auto");

    if (this.hudShifts) this.hudShifts.textContent = "0";
    if (this.hudCodec) {
      this.hudCodec.textContent = isHls ? "H.264 / HLS ABR (v1.5+)" : "Direct Zero-Copy";
    }

    if (this.seekRange) this.seekRange.value = "0";
    if (this.currentTimeEl) this.currentTimeEl.textContent = "00:00";
    if (this.durationEl) this.durationEl.textContent = "00:00";

    // Preserved Persistent Settings Re-application
    this.applyStoredSettings();

    if (this.resolutionSelect) {
      this.resolutionSelect.innerHTML =
        "<option value='-1' selected id='opt-resolution-auto'>Auto ABR (Adaptive)</option>";
    }

    if (isHls && Hls.isSupported()) {
      // Cinema-Grade Hls.js v1.5+ configuration
      this.hls = new Hls({
        capLevelToPlayerSize: true, // Dynamically cap to viewport only in Auto mode
        maxBufferLength: 30, // 30s forward buffer horizon
        maxMaxBufferLength: 60, // 60s max buffer cap
        maxBufferSize: 60 * 1024 * 1024, // 60 MB buffer ceiling
        maxBufferHole: 0.5,
        backBufferLength: 30, // 30s backward buffer preservation for instant seek back
        lowLatencyMode: true,

        // EWMA Sliding Window Bandwidth Estimation & ABR Tuning
        abrEwmaFastLive: 3.0,
        abrEwmaSlowLive: 9.0,
        abrEwmaFastVoD: 3.0,
        abrEwmaSlowVoD: 9.0,
        abrEwmaDefaultEstimate: 5000000, // 5 Mbps baseline to prevent starting at lowest rendition
        abrBandWidthFactor: 0.85, // 15% safety headroom
        abrBandWidthUpFactor: 0.70, // 30% headroom required to step up ladder
        abrMaxWithRealBitrate: true,

        fragLoadingTimeOut: 20000,
        manifestLoadingTimeOut: 15000,
        levelLoadingTimeOut: 15000,
      });

      this.hls.loadSource(url);
      this.hls.attachMedia(this.videoElement);

      this.hls.on(Hls.Events.MANIFEST_PARSED, () => {
        this.availableLevels = this.parseLevels();
        this.populateResolutionPicker(this.availableLevels);
        this.onLevelsLoaded?.(this.availableLevels);

        // If user preferred a specific resolution level, restore it
        const prefRes = this.settingsManager.getPreferredResolution();
        if (prefRes !== -1 && prefRes < this.availableLevels.length) {
          this.setLevel(prefRes);
        }
      });

      this.hls.on(Hls.Events.LEVEL_SWITCHED, (_, data) => {
        this.handleLevelSwitched(data.level);
      });

      this.hls.on(Hls.Events.FRAG_LOADED, (_, data) => {
        const stats = data.frag.stats as any;
        const bw = Math.round((stats.bandwidth || this.hls?.bandwidthEstimate || 0) / 1000);
        this.hudBandwidth.textContent = `${bw} kbps`;
      });

      // Dynamic Subtitle Tracks Discovery & Restoration
      this.hls.on(Hls.Events.SUBTITLE_TRACKS_UPDATED, (_, data) => {
        this.populateSubtitleTracks(data.subtitleTracks);
        const s = this.settingsManager.getSettings();
        this.applySubtitleTrack(s.subtitleLanguage, s.subtitlesEnabled);
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
              console.error(
                "[HLS] Unrecoverable fatal error encountered, falling back to direct stream:",
                data
              );
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
      this.updateQualityBadge("NATIVE", "auto");
    } else {
      // Zero-Copy Direct Stream Fallback
      this.videoElement.src = url;
      this.hudRendition.textContent = "Direct (Zero-Copy)";
      this.updateQualityBadge("DIRECT", "manual");
    }

    this.videoElement.play().catch(() => {
      // Autoplay blocked by browser policy until user gesture
    });

    this.startTelemetryLoop();
  }

  private parseLevels(): RenditionInfo[] {
    if (!this.hls || !this.hls.levels) return [];
    return this.hls.levels.map((lvl, idx) => {
      const height = lvl.height || 0;
      const width = lvl.width || 0;
      const bitrate = lvl.bitrate || 0;
      const frameRate = lvl.frameRate;
      const codecs = lvl.videoCodec || (lvl.attrs as any)?.CODECS;

      let label = `${height}p`;
      if (height >= 2160) label = "4K 2160p";
      else if (height >= 1440) label = "2K 1440p";
      else if (height >= 1080) label = "1080p FHD";
      else if (height >= 720) label = "720p HD";
      else if (height >= 480) label = "480p SD";
      else if (height >= 360) label = "360p Mobile";

      const bitrateMbps = (bitrate / 1_000_000).toFixed(1);
      const bitrateKbps = Math.round(bitrate / 1000);
      const bitrateStr = bitrate >= 1_000_000 ? `${bitrateMbps} Mbps` : `${bitrateKbps} kbps`;
      const fpsStr = frameRate && frameRate > 30 ? ` ${Math.round(frameRate)}fps` : "";
      const name = `${label}${fpsStr} (${bitrateStr})`;

      return {
        index: idx,
        height,
        width,
        bitrate,
        frameRate,
        codecs,
        name,
        shortName: `${height}p`,
        label,
        isAuto: false,
      };
    });
  }

  private populateResolutionPicker(levels: RenditionInfo[]) {
    if (!this.resolutionSelect) return;
    this.resolutionSelect.innerHTML = "";

    // 1. Auto ABR Option
    const autoOpt = document.createElement("option");
    autoOpt.value = "-1";
    autoOpt.id = "opt-resolution-auto";
    autoOpt.textContent = "Auto ABR (Adaptive)";
    autoOpt.selected = !this.isManualResolution;
    this.resolutionSelect.appendChild(autoOpt);

    // 2. Manual Rendition Levels
    levels.forEach((lvl) => {
      const opt = document.createElement("option");
      opt.value = lvl.index.toString();
      opt.textContent = lvl.name;
      if (this.isManualResolution && this.hls?.currentLevel === lvl.index) {
        opt.selected = true;
      }
      this.resolutionSelect!.appendChild(opt);
    });
  }

  private populateSubtitleTracks(tracks: any[]) {
    if (!this.subtitlesSelect) return;
    const currentVal = this.subtitlesSelect.value;
    this.subtitlesSelect.innerHTML = "<option value='off'>Subtitles: Off</option>";

    if (tracks && tracks.length > 0) {
      const autoOpt = document.createElement("option");
      autoOpt.value = "auto";
      autoOpt.textContent = "Auto Track";
      this.subtitlesSelect.appendChild(autoOpt);

      tracks.forEach((track, idx) => {
        const opt = document.createElement("option");
        opt.value = track.lang || `track_${idx}`;
        opt.textContent = `${track.name || track.lang || `Track ${idx + 1}`}${
          track.lang ? ` [${track.lang.toUpperCase()}]` : ""
        }`;
        this.subtitlesSelect!.appendChild(opt);
      });
    }

    if (currentVal) {
      this.subtitlesSelect.value = currentVal;
    }
  }

  public applySubtitleTrack(lang: string, enabled: boolean) {
    if (!enabled || lang === "off") {
      if (this.hls) {
        this.hls.subtitleTrack = -1;
      }
      for (let i = 0; i < this.videoElement.textTracks.length; i++) {
        this.videoElement.textTracks[i].mode = "disabled";
      }
      return;
    }

    if (this.hls && this.hls.subtitleTracks && this.hls.subtitleTracks.length > 0) {
      if (lang === "auto") {
        this.hls.subtitleTrack = 0;
      } else {
        const matchIdx = this.hls.subtitleTracks.findIndex(
          (t) => t.lang?.toLowerCase() === lang.toLowerCase()
        );
        this.hls.subtitleTrack = matchIdx >= 0 ? matchIdx : 0;
      }
    } else {
      for (let i = 0; i < this.videoElement.textTracks.length; i++) {
        const track = this.videoElement.textTracks[i];
        if (track.language.toLowerCase() === lang.toLowerCase()) {
          track.mode = "showing";
        } else {
          track.mode = "disabled";
        }
      }
    }
  }

  private handleLevelSwitched(levelIndex: number) {
    this.abrShiftCount++;
    if (this.hudShifts) {
      this.hudShifts.textContent = this.abrShiftCount.toString();
    }

    const level = this.hls?.levels[levelIndex];
    if (level) {
      const info: RenditionInfo = this.availableLevels.find((l) => l.index === levelIndex) || {
        index: levelIndex,
        height: level.height,
        width: level.width,
        bitrate: level.bitrate,
        name: `${level.height}p (${Math.round(level.bitrate / 1000)}k)`,
        shortName: `${level.height}p`,
        label: `${level.height}p`,
      };
      this.currentActiveLevel = info;

      if (!this.isManualResolution) {
        // Auto ABR active
        const autoOpt = document.getElementById("opt-resolution-auto");
        if (autoOpt) {
          autoOpt.textContent = `Auto (${info.shortName})`;
        }
        this.updateQualityBadge(`AUTO · ${info.shortName}`, "auto");
        this.hudRendition.textContent = `${info.name} [Auto]`;
        this.onLevelSwitched?.(info);
      } else {
        // Manual override locked
        this.updateQualityBadge(`${info.shortName}`, "manual");
        this.hudRendition.textContent = `${info.name} [Manual]`;
        this.onLevelSwitched?.(info);
      }
    }
  }

  public setLevel(levelIndex: number) {
    if (!this.hls) return;

    if (levelIndex === -1) {
      // Re-enable Auto ABR
      this.isManualResolution = false;
      this.hls.currentLevel = -1; // -1 instructs Hls.js to use automatic ABR ladder
      this.hls.capLevelToPlayerSize = true;
      if (this.resolutionSelect) {
        this.resolutionSelect.value = "-1";
      }
      this.updateQualityBadge("AUTO", "auto");
      this.showQualityToast("Auto ABR (Adaptive)");
      this.hudRendition.textContent = "Auto ABR";
      this.onLevelSwitched?.(null);
    } else {
      // Manual Quality Selection
      const level = this.hls.levels[levelIndex];
      if (level) {
        this.isManualResolution = true;
        this.hls.currentLevel = levelIndex; // Lock to specific rendition level
        this.hls.loadLevel = levelIndex;
        this.hls.capLevelToPlayerSize = false; // Never downscale user's explicit quality choice
        if (this.resolutionSelect) {
          this.resolutionSelect.value = levelIndex.toString();
        }
        const info = this.availableLevels.find((l) => l.index === levelIndex) || {
          index: levelIndex,
          height: level.height,
          bitrate: level.bitrate,
          name: `${level.height}p (${Math.round(level.bitrate / 1000)}k)`,
          shortName: `${level.height}p`,
          label: `${level.height}p`,
        };
        this.currentActiveLevel = info;
        this.updateQualityBadge(`${info.shortName}`, "manual");
        this.showQualityToast(`Resolution: ${info.name}`);
        this.hudRendition.textContent = `${info.name} [Manual]`;
        this.onLevelSwitched?.(info);
      }
    }
  }

  public setAutoLevel() {
    this.setLevel(-1);
  }

  public isAuto(): boolean {
    return !this.isManualResolution;
  }

  public getCurrentLevel(): number {
    return this.hls ? this.hls.currentLevel : -1;
  }

  public getLevels(): RenditionInfo[] {
    return this.availableLevels;
  }

  public getCurrentRendition(): RenditionInfo | null {
    return this.currentActiveLevel;
  }

  public getHls(): Hls | null {
    return this.hls;
  }

  public getSettingsManager(): PlaybackSettingsManager {
    return this.settingsManager;
  }

  public showQualityToast(message: string) {
    if (!this.qualityToastEl) return;
    if (this.toastTimer !== null) {
      clearTimeout(this.toastTimer);
    }
    this.qualityToastEl.textContent = message;
    this.qualityToastEl.classList.remove("hidden");
    this.toastTimer = window.setTimeout(() => {
      this.qualityToastEl?.classList.add("hidden");
      this.toastTimer = null;
    }, 1600);
  }

  private updateQualityBadge(text: string, mode: "auto" | "manual") {
    if (!this.qualityBadgeEl) return;
    this.qualityBadgeEl.textContent = text;
    this.qualityBadgeEl.className = `quality-badge ${mode}`;
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
      // 1. Buffer health calculation & adaptive coloration
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

        // Color coding based on buffer margin
        if (forwardBuffer >= 15.0) {
          this.hudBuffer.className = "hud-val-good";
        } else if (forwardBuffer >= 6.0) {
          this.hudBuffer.className = "hud-val-warn";
        } else {
          this.hudBuffer.className = "hud-val-danger";
        }

        // Emergency buffer starvation prevention (<6s) in Auto ABR mode
        if (
          !this.isManualResolution &&
          forwardBuffer < 6.0 &&
          this.hls &&
          this.hls.currentLevel > 0
        ) {
          console.warn("[TELEMETRY] Buffer starvation alert (<6s). Downgrading rendition ladder.");
          this.hls.nextLevel = 0;
        }
      }

      // 2. Dropped frames calculation & GPU decode throttling detection
      if ("getVideoPlaybackQuality" in this.videoElement) {
        const quality = this.videoElement.getVideoPlaybackQuality();
        const dropped = quality.droppedVideoFrames;
        const total = quality.totalVideoFrames;
        const dropPct = total > 0 ? (dropped / total) * 100 : 0;

        this.hudDropped.textContent = `${dropped} (${dropPct.toFixed(1)}%)`;

        if (dropPct > 5.0) {
          this.hudDropped.className = "hud-val-danger";
          if (!this.isManualResolution && this.hls && this.hls.currentLevel > 0) {
            console.warn(
              `[TELEMETRY] Excessive frame drop rate (${dropPct.toFixed(1)}%). Downscaling rendition.`
            );
            this.hls.nextLevel = Math.max(0, this.hls.currentLevel - 1);
          }
        } else if (dropPct > 1.0) {
          this.hudDropped.className = "hud-val-warn";
        } else {
          this.hudDropped.className = "hud-val-good";
        }
      }

      // 3. Bandwidth update
      if (this.hls) {
        const bw = Math.round((this.hls.bandwidthEstimate || 0) / 1000);
        this.hudBandwidth.textContent = `${bw} kbps`;
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
    if (this.toastTimer !== null) {
      clearTimeout(this.toastTimer);
      this.toastTimer = null;
    }
    if (this.hls) {
      this.hls.destroy();
      this.hls = null;
    }
    this.videoElement.pause();
    this.videoElement.removeAttribute("src");
    this.videoElement.load();
    this.videoContainerEl?.classList.remove("theater-mode");
    this.isManualResolution = false;
    this.currentActiveLevel = null;
    this.availableLevels = [];
  }
}
