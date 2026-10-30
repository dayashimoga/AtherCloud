/**
 * AetherStream Persistent Playback Settings Manager
 * Stores user preferences across video sessions and browser reloads via localStorage:
 * - Volume level (0.0 to 1.0)
 * - Mute state (boolean)
 * - Playback rate / speed (0.5x to 2.0x)
 * - Subtitle language and enabled state
 * - Preferred stream resolution (-1 for Auto ABR, or target height e.g. 1080)
 */

export interface PlaybackSettings {
  volume: number;
  muted: boolean;
  playbackRate: number;
  subtitleLanguage: string;
  subtitlesEnabled: boolean;
  preferredResolution: number;
}

export const PLAYBACK_SETTINGS_STORAGE_KEY = "aetherstream_playback_settings_v1";

export const DEFAULT_PLAYBACK_SETTINGS: PlaybackSettings = {
  volume: 1.0,
  muted: false,
  playbackRate: 1.0,
  subtitleLanguage: "off",
  subtitlesEnabled: false,
  preferredResolution: -1,
};

export class PlaybackSettingsManager {
  private static instance: PlaybackSettingsManager;
  private settings: PlaybackSettings;
  private listeners: Array<(settings: PlaybackSettings) => void> = [];
  private memoryFallback = false;

  private constructor() {
    this.settings = this.load();
  }

  public static getInstance(): PlaybackSettingsManager {
    if (!PlaybackSettingsManager.instance) {
      PlaybackSettingsManager.instance = new PlaybackSettingsManager();
    }
    return PlaybackSettingsManager.instance;
  }

  private load(): PlaybackSettings {
    try {
      if (typeof window !== "undefined" && window.localStorage) {
        const raw = window.localStorage.getItem(PLAYBACK_SETTINGS_STORAGE_KEY);
        if (raw) {
          const parsed = JSON.parse(raw);
          return {
            volume:
              typeof parsed.volume === "number" && !isNaN(parsed.volume)
                ? Math.max(0, Math.min(1, parsed.volume))
                : DEFAULT_PLAYBACK_SETTINGS.volume,
            muted:
              typeof parsed.muted === "boolean"
                ? parsed.muted
                : DEFAULT_PLAYBACK_SETTINGS.muted,
            playbackRate:
              typeof parsed.playbackRate === "number" && !isNaN(parsed.playbackRate)
                ? parsed.playbackRate
                : DEFAULT_PLAYBACK_SETTINGS.playbackRate,
            subtitleLanguage:
              typeof parsed.subtitleLanguage === "string"
                ? parsed.subtitleLanguage
                : DEFAULT_PLAYBACK_SETTINGS.subtitleLanguage,
            subtitlesEnabled:
              typeof parsed.subtitlesEnabled === "boolean"
                ? parsed.subtitlesEnabled
                : DEFAULT_PLAYBACK_SETTINGS.subtitlesEnabled,
            preferredResolution:
              typeof parsed.preferredResolution === "number" && !isNaN(parsed.preferredResolution)
                ? parsed.preferredResolution
                : DEFAULT_PLAYBACK_SETTINGS.preferredResolution,
          };
        }
      }
    } catch {
      this.memoryFallback = true;
    }
    return { ...DEFAULT_PLAYBACK_SETTINGS };
  }

  public getSettings(): Readonly<PlaybackSettings> {
    return this.settings;
  }

  public getVolume(): number {
    return this.settings.volume;
  }

  public isMuted(): boolean {
    return this.settings.muted;
  }

  public getPlaybackRate(): number {
    return this.settings.playbackRate;
  }

  public getSubtitleLanguage(): string {
    return this.settings.subtitleLanguage;
  }

  public areSubtitlesEnabled(): boolean {
    return this.settings.subtitlesEnabled;
  }

  public getPreferredResolution(): number {
    return this.settings.preferredResolution;
  }

  public setVolume(volume: number): void {
    const clamped = Math.max(0, Math.min(1, volume));
    this.update({
      volume: clamped,
      muted: clamped === 0 ? true : this.settings.muted,
    });
  }

  public setMuted(muted: boolean): void {
    this.update({ muted });
  }

  public setPlaybackRate(playbackRate: number): void {
    const validRates = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];
    const rate = validRates.includes(playbackRate) ? playbackRate : 1.0;
    this.update({ playbackRate: rate });
  }

  public setSubtitlePreference(subtitleLanguage: string, subtitlesEnabled?: boolean): void {
    const isOff = subtitleLanguage === "off";
    const enabled = subtitlesEnabled !== undefined ? subtitlesEnabled : !isOff;
    this.update({
      subtitleLanguage,
      subtitlesEnabled: isOff ? false : enabled,
    });
  }

  public toggleSubtitles(): boolean {
    const newEnabled = !this.settings.subtitlesEnabled;
    this.update({
      subtitlesEnabled: newEnabled,
      subtitleLanguage:
        newEnabled && this.settings.subtitleLanguage === "off"
          ? "en"
          : this.settings.subtitleLanguage,
    });
    return newEnabled;
  }

  public setPreferredResolution(preferredResolution: number): void {
    this.update({ preferredResolution });
  }

  public update(partial: Partial<PlaybackSettings>): void {
    this.settings = { ...this.settings, ...partial };
    this.persist();
    this.notify();
  }

  public reset(): void {
    this.settings = { ...DEFAULT_PLAYBACK_SETTINGS };
    try {
      if (typeof window !== "undefined" && window.localStorage) {
        window.localStorage.removeItem(PLAYBACK_SETTINGS_STORAGE_KEY);
      }
    } catch {
      // Ignored
    }
    this.notify();
  }

  private persist(): void {
    if (this.memoryFallback) return;
    try {
      if (typeof window !== "undefined" && window.localStorage) {
        window.localStorage.setItem(
          PLAYBACK_SETTINGS_STORAGE_KEY,
          JSON.stringify(this.settings)
        );
      }
    } catch {
      this.memoryFallback = true;
    }
  }

  public subscribe(listener: (settings: PlaybackSettings) => void): () => void {
    this.listeners.push(listener);
    return () => {
      this.listeners = this.listeners.filter((l) => l !== listener);
    };
  }

  private notify(): void {
    for (const listener of this.listeners) {
      try {
        listener(this.settings);
      } catch (err) {
        console.error("[PlaybackSettings] Listener notification error:", err);
      }
    }
  }
}

