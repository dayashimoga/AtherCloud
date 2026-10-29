import "./app.css";
import { AetherApi } from "./services/api";
import { FileExplorerComponent } from "./components/FileExplorer";
import { VideoPlayerComponent } from "./components/VideoPlayer";
import { ShareModalComponent } from "./components/ShareModal";

class AetherApp {
  private fileExplorer: FileExplorerComponent;
  private videoPlayer: VideoPlayerComponent;
  private shareModal: ShareModalComponent;
  private playerModalEl: HTMLElement;
  private videoTitleEl: HTMLElement;
  private closePlayerBtn: HTMLButtonElement;
  private refreshBtn: HTMLButtonElement;
  private hwAccelBadge: HTMLElement;

  constructor() {
    this.fileExplorer = new FileExplorerComponent();
    this.videoPlayer = new VideoPlayerComponent();
    this.shareModal = new ShareModalComponent();
    this.playerModalEl = document.getElementById("player-modal")!;
    this.videoTitleEl = document.getElementById("video-title")!;
    this.closePlayerBtn = document.getElementById("close-player-btn") as HTMLButtonElement;
    this.refreshBtn = document.getElementById("refresh-btn") as HTMLButtonElement;
    this.hwAccelBadge = document.getElementById("hw-accel-badge")!;

    this.bindEvents();
    this.init();
  }

  private bindEvents() {
    this.closePlayerBtn.onclick = () => this.closePlayer();
    this.playerModalEl.querySelector(".modal-backdrop")!.addEventListener("click", () => this.closePlayer());
    this.refreshBtn.onclick = () => this.loadMounts();

    this.fileExplorer.onPlayMedia = (mount, path, isHls) => {
      this.openPlayer(mount, path, isHls);
    };

    this.fileExplorer.onShareItem = (mount, path) => {
      this.shareModal.show(mount, path);
    };
  }

  private async init() {
    try {
      const health = await AetherApi.getHealth();
      this.hwAccelBadge.textContent = `Accel: ${health.hardware_acceleration}`;
    } catch {
      this.hwAccelBadge.textContent = "Offline / Server Disconnected";
      this.hwAccelBadge.style.color = "var(--danger)";
    }

    await this.loadMounts();
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
    if ("serviceWorker" in navigator && window.location.protocol === "https:") {
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
