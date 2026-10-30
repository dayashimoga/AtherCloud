import "./app.css";
import { AetherApi } from "./services/api";
import { SessionManager, ThemeMode } from "./services/session";
import { FileExplorerComponent } from "./components/FileExplorer";
import { VideoPlayerComponent } from "./components/VideoPlayer";
import { ShareModalComponent } from "./components/ShareModal";

class AetherApp {
  private fileExplorer: FileExplorerComponent;
  private videoPlayer: VideoPlayerComponent;
  private shareModal: ShareModalComponent;
  private session: SessionManager;

  private playerModalEl: HTMLElement;
  private videoTitleEl: HTMLElement;
  private closePlayerBtn: HTMLButtonElement;
  private refreshBtn: HTMLButtonElement;
  private hwAccelBadge: HTMLElement;

  // Theme & Session Settings
  private themeToggleBtn: HTMLButtonElement;
  private settingsBtn: HTMLButtonElement;
  private settingsModalEl: HTMLElement;
  private closeSettingsBtn: HTMLButtonElement;
  private saveSettingsBtn: HTMLButtonElement;
  private clearSessionBtn: HTMLButtonElement;
  private serverUrlInput: HTMLInputElement;
  private authTokenInput: HTMLInputElement;
  private themeSelect: HTMLSelectElement;

  constructor() {
    this.session = SessionManager.getInstance();
    this.fileExplorer = new FileExplorerComponent();
    this.videoPlayer = new VideoPlayerComponent();
    this.shareModal = new ShareModalComponent();

    this.playerModalEl = document.getElementById("player-modal")!;
    this.videoTitleEl = document.getElementById("video-title")!;
    this.closePlayerBtn = document.getElementById("close-player-btn") as HTMLButtonElement;
    this.refreshBtn = document.getElementById("refresh-btn") as HTMLButtonElement;
    this.hwAccelBadge = document.getElementById("hw-accel-badge")!;

    this.themeToggleBtn = document.getElementById("theme-toggle-btn") as HTMLButtonElement;
    this.settingsBtn = document.getElementById("settings-btn") as HTMLButtonElement;
    this.settingsModalEl = document.getElementById("settings-modal")!;
    this.closeSettingsBtn = document.getElementById("close-settings-btn") as HTMLButtonElement;
    this.saveSettingsBtn = document.getElementById("save-settings-btn") as HTMLButtonElement;
    this.clearSessionBtn = document.getElementById("clear-session-btn") as HTMLButtonElement;
    this.serverUrlInput = document.getElementById("settings-server-url") as HTMLInputElement;
    this.authTokenInput = document.getElementById("settings-auth-token") as HTMLInputElement;
    this.themeSelect = document.getElementById("settings-theme") as HTMLSelectElement;

    this.bindEvents();
    this.init();
  }

  private bindEvents() {
    this.closePlayerBtn.onclick = () => this.closePlayer();
    this.playerModalEl.querySelector(".modal-backdrop")!.addEventListener("click", () => this.closePlayer());
    this.refreshBtn.onclick = () => {
      this.loadMounts();
      this.loadDrives();
    };

    // Theme Toggle
    this.themeToggleBtn.onclick = () => {
      const newTheme = this.session.toggleTheme();
      this.updateThemeButtonIcon(newTheme);
    };

    // Settings Modal
    this.settingsBtn.onclick = () => this.openSettings();
    this.closeSettingsBtn.onclick = () => this.closeSettings();
    this.settingsModalEl.querySelector(".modal-backdrop")!.addEventListener("click", () => this.closeSettings());

    this.saveSettingsBtn.onclick = () => {
      const serverUrl = this.serverUrlInput.value.trim();
      const token = this.authTokenInput.value.trim() || null;
      const theme = this.themeSelect.value as ThemeMode;

      this.session.saveState({ serverUrl, token, theme });
      this.updateThemeButtonIcon(theme);
      this.closeSettings();
      this.loadMounts();
      this.loadDrives();
    };

    this.clearSessionBtn.onclick = () => {
      if (confirm("Reset all stored session tokens and client preferences?")) {
        this.session.clearSession();
        this.serverUrlInput.value = "";
        this.authTokenInput.value = "";
        this.themeSelect.value = "dark";
        this.updateThemeButtonIcon("dark");
        this.closeSettings();
        this.loadMounts();
      }
    };

    this.fileExplorer.onPlayMedia = (mount, path, isHls) => {
      this.openPlayer(mount, path, isHls);
    };

    this.fileExplorer.onShareItem = (mount, path) => {
      this.shareModal.show(mount, path);
    };
  }

  private updateThemeButtonIcon(theme: ThemeMode) {
    if (theme === "light") {
      this.themeToggleBtn.innerHTML = "&#9728;"; // Sun
      this.themeToggleBtn.title = "Current: Light Theme (Click for Dark)";
    } else {
      this.themeToggleBtn.innerHTML = "&#127769;"; // Moon
      this.themeToggleBtn.title = "Current: Dark Theme (Click for Light)";
    }
  }

  private openSettings() {
    const config = this.session.getState();
    this.serverUrlInput.value = config.serverUrl;
    this.authTokenInput.value = config.token || "";
    this.themeSelect.value = config.theme;
    this.settingsModalEl.classList.remove("hidden");
  }

  private closeSettings() {
    this.settingsModalEl.classList.add("hidden");
  }

  private async init() {
    this.updateThemeButtonIcon(this.session.getTheme());

    try {
      const health = await AetherApi.getHealth();
      this.hwAccelBadge.textContent = `Accel: ${health.hardware_acceleration}`;
    } catch {
      this.hwAccelBadge.textContent = "Offline / Server Disconnected";
      this.hwAccelBadge.style.color = "var(--danger)";
    }

    await Promise.allSettled([this.loadMounts(), this.loadDrives()]);
    this.registerServiceWorker();
  }

  private async loadMounts() {
    try {
      const mounts = await AetherApi.getMounts();
      this.fileExplorer.renderMounts(mounts);
    } catch (err: any) {
      console.error("Failed to load mounts:", err);
    }
  }

  private async loadDrives() {
    try {
      const drives = await AetherApi.getSystemDrives();
      this.fileExplorer.renderDrives(drives);
    } catch (err: any) {
      console.error("Failed to load system drives:", err);
    }
  }

  private openPlayer(mount: string, path: string, isHls: boolean) {
    const fileName = path.split("/").pop() || path;
    this.videoTitleEl.textContent = `${fileName} (${isHls ? "HLS Adaptive" : "Direct Stream"})`;
    this.playerModalEl.classList.remove("hidden");

    const streamUrl = isHls
      ? AetherApi.getHlsMasterUrl(mount, path)
      : AetherApi.getStreamUrl(mount, path);

    this.videoPlayer.loadSource(streamUrl, isHls);
  }

  private closePlayer() {
    this.videoPlayer.destroy();
    this.playerModalEl.classList.add("hidden");
  }

  private registerServiceWorker() {
    if (
      "serviceWorker" in navigator &&
      (window.location.protocol === "https:" ||
        window.location.hostname === "localhost" ||
        window.location.hostname === "127.0.0.1")
    ) {
      navigator.serviceWorker
        .register("/sw.js")
        .then(() => console.log("[PWA] Service Worker registered"))
        .catch((err) => console.warn("[PWA] Service Worker registration failed:", err));
    }
  }
}

// Boot application when DOM is ready
window.addEventListener("DOMContentLoaded", () => {
  new AetherApp();
});
