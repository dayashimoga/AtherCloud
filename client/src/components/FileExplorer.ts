import { AetherApi, FileItem, MountItem } from "../services/api";

export class FileExplorerComponent {
  private mountListEl: HTMLElement;
  private fileGridEl: HTMLElement;
  private breadcrumbEl: HTMLElement;
  private currentMount: string | null = null;
  private currentPath = "";

  public onPlayMedia: ((mount: string, path: string, isHls: boolean) => void) | null = null;
  public onShareItem: ((mount: string, path: string) => void) | null = null;

  constructor() {
    this.mountListEl = document.getElementById("mount-list")!;
    this.fileGridEl = document.getElementById("file-grid")!;
    this.breadcrumbEl = document.getElementById("breadcrumb")!;
  }

  renderMounts(mounts: MountItem[]) {
    this.mountListEl.innerHTML = "";
    if (mounts.length === 0) {
      this.mountListEl.innerHTML = "<li class='loading'>No mounts configured</li>";
      return;
    }

    mounts.forEach((m, idx) => {
      const li = document.createElement("li");
      li.className = `mount-item ${idx === 0 ? "active" : ""}`;
      li.textContent = m.id;
      li.title = m.description || m.path_display;
      li.onclick = () => {
        document.querySelectorAll(".mount-item").forEach((el) => el.classList.remove("active"));
        li.classList.add("active");
        this.loadDirectory(m.id, "");
      };
      this.mountListEl.appendChild(li);
    });

    if (mounts.length > 0) {
      this.loadDirectory(mounts[0].id, "");
    }
  }

  async loadDirectory(mount: string, path = "") {
    this.currentMount = mount;
    this.currentPath = path;
    this.updateBreadcrumb();
    this.fileGridEl.innerHTML = "<div class='loading'>Loading directory contents...</div>";

    try {
      const res = await AetherApi.browse(mount, path);
      this.renderFiles(res.items);
    } catch (err: any) {
      this.fileGridEl.innerHTML = `<div class='loading' style='color:var(--danger)'>${err.message}</div>`;
    }
  }

  private updateBreadcrumb() {
    const parts = this.currentPath.split("/").filter(Boolean);
    this.breadcrumbEl.innerHTML = "";

    const rootSpan = document.createElement("span");
    rootSpan.className = "breadcrumb-link";
    rootSpan.textContent = `[${this.currentMount}] /`;
    rootSpan.style.cursor = "pointer";
    rootSpan.onclick = () => this.loadDirectory(this.currentMount!, "");
    this.breadcrumbEl.appendChild(rootSpan);

    let accum = "";
    parts.forEach((part, i) => {
      accum += (i === 0 ? "" : "/") + part;
      const target = accum;
      const separator = document.createTextNode(" ");
      const seg = document.createElement("span");
      seg.className = "breadcrumb-link";
      seg.textContent = `${part} /`;
      seg.style.cursor = "pointer";
      seg.onclick = () => this.loadDirectory(this.currentMount!, target);
      this.breadcrumbEl.appendChild(separator);
      this.breadcrumbEl.appendChild(seg);
    });
  }

  private renderFiles(items: FileItem[]) {
    this.fileGridEl.innerHTML = "";
    if (items.length === 0) {
      this.fileGridEl.innerHTML = "<div class='loading'>This directory is empty.</div>";
      return;
    }

    items.forEach((item) => {
      const card = document.createElement("div");
      card.className = "file-card";

      const icon = document.createElement("div");
      icon.className = "file-icon";
      icon.innerHTML = item.is_dir ? "&#128193;" : this.getMediaIcon(item.name);

      const name = document.createElement("div");
      name.className = "file-name";
      name.textContent = item.name;

      const meta = document.createElement("div");
      meta.className = "file-meta";
      meta.textContent = item.is_dir ? "Folder" : this.formatBytes(item.size);

      const actions = document.createElement("div");
      actions.className = "card-actions";

      if (item.is_dir) {
        card.onclick = () => {
          const next = this.currentPath ? `${this.currentPath}/${item.name}` : item.name;
          this.loadDirectory(this.currentMount!, next);
        };
      } else {
        const isVideo = this.isVideoFile(item.name);
        if (isVideo) {
          const playBtn = document.createElement("button");
          playBtn.className = "btn btn-primary btn-sm";
          playBtn.textContent = "Play Direct";
          playBtn.onclick = (e) => {
            e.stopPropagation();
            const filePath = this.currentPath ? `${this.currentPath}/${item.name}` : item.name;
            this.onPlayMedia?.(this.currentMount!, filePath, false);
          };
          actions.appendChild(playBtn);

          const hlsBtn = document.createElement("button");
          hlsBtn.className = "btn btn-secondary btn-sm";
          hlsBtn.textContent = "HLS ABR";
          hlsBtn.onclick = (e) => {
            e.stopPropagation();
            const filePath = this.currentPath ? `${this.currentPath}/${item.name}` : item.name;
            this.onPlayMedia?.(this.currentMount!, filePath, true);
          };
          actions.appendChild(hlsBtn);
        }

        const shareBtn = document.createElement("button");
        shareBtn.className = "btn btn-secondary btn-sm";
        shareBtn.textContent = "Share";
        shareBtn.onclick = (e) => {
          e.stopPropagation();
          const filePath = this.currentPath ? `${this.currentPath}/${item.name}` : item.name;
          this.onShareItem?.(this.currentMount!, filePath);
        };
        actions.appendChild(shareBtn);
      }

      card.appendChild(icon);
      card.appendChild(name);
      card.appendChild(meta);
      if (!item.is_dir) {
        card.appendChild(actions);
      }
      this.fileGridEl.appendChild(card);
    });
  }

  private isVideoFile(fileName: string): boolean {
    return /\.(mp4|mkv|webm|mov|m4v|avi)$/i.test(fileName);
  }

  private getMediaIcon(fileName: string): string {
    if (this.isVideoFile(fileName)) return "&#127916;"; // film
    if (/\.(mp3|flac|wav|aac|ogg|m4a)$/i.test(fileName)) return "&#127925;"; // audio
    if (/\.(jpg|jpeg|png|gif|webp|svg)$/i.test(fileName)) return "&#128444;"; // image
    return "&#128196;"; // document
  }

  private formatBytes(bytes: number): string {
    if (bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB", "TB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
  }
}
