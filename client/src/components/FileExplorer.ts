import { AetherApi, DiscoveredDriveItem, FileItem, MountItem } from "../services/api";

export type ViewMode = "grid" | "list";

export class FileExplorerComponent {
  private mountListEl: HTMLElement;
  private driveListEl: HTMLElement | null = null;
  private mountCountEl: HTMLElement | null = null;
  private driveCountEl: HTMLElement | null = null;
  private fileGridEl: HTMLElement;
  private fileListEl: HTMLElement | null = null;
  private breadcrumbEl: HTMLElement;
  private searchInputEl: HTMLInputElement | null = null;
  private clearSearchBtn: HTMLButtonElement | null = null;
  private viewGridBtn: HTMLButtonElement | null = null;
  private viewListBtn: HTMLButtonElement | null = null;

  private currentMount: string | null = null;
  private currentPath = "";
  private currentItems: FileItem[] = [];
  private searchQuery = "";
  private viewMode: ViewMode = "grid";

  public onPlayMedia: ((mount: string, path: string, isHls: boolean) => void) | null = null;
  public onShareItem: ((mount: string, path: string) => void) | null = null;

  constructor() {
    this.mountListEl = document.getElementById("mount-list")!;
    this.driveListEl = document.getElementById("drive-list");
    this.mountCountEl = document.getElementById("mount-count");
    this.driveCountEl = document.getElementById("drive-count");
    this.fileGridEl = document.getElementById("file-grid")!;
    this.fileListEl = document.getElementById("file-list");
    this.breadcrumbEl = document.getElementById("breadcrumb")!;

    this.searchInputEl = document.getElementById("file-search-input") as HTMLInputElement | null;
    this.clearSearchBtn = document.getElementById("clear-search-btn") as HTMLButtonElement | null;
    this.viewGridBtn = document.getElementById("view-grid-btn") as HTMLButtonElement | null;
    this.viewListBtn = document.getElementById("view-list-btn") as HTMLButtonElement | null;

    this.bindControls();
  }

  private bindControls() {
    if (this.searchInputEl) {
      this.searchInputEl.addEventListener("input", (e) => {
        const val = (e.target as HTMLInputElement).value;
        this.setSearchQuery(val);
      });
    }

    if (this.clearSearchBtn) {
      this.clearSearchBtn.addEventListener("click", () => {
        if (this.searchInputEl) {
          this.searchInputEl.value = "";
          this.setSearchQuery("");
          this.searchInputEl.focus();
        }
      });
    }

    if (this.viewGridBtn) {
      this.viewGridBtn.addEventListener("click", () => this.setViewMode("grid"));
    }

    if (this.viewListBtn) {
      this.viewListBtn.addEventListener("click", () => this.setViewMode("list"));
    }
  }

  public getViewMode(): ViewMode {
    return this.viewMode;
  }

  public setViewMode(mode: ViewMode) {
    this.viewMode = mode;
    if (this.viewGridBtn && this.viewListBtn) {
      if (mode === "grid") {
        this.viewGridBtn.classList.add("active");
        this.viewListBtn.classList.remove("active");
      } else {
        this.viewListBtn.classList.add("active");
        this.viewGridBtn.classList.remove("active");
      }
    }

    if (this.fileGridEl && this.fileListEl) {
      if (mode === "grid") {
        this.fileGridEl.classList.remove("hidden");
        this.fileListEl.classList.add("hidden");
      } else {
        this.fileGridEl.classList.add("hidden");
        this.fileListEl.classList.remove("hidden");
      }
    }
  }

  public setSearchQuery(query: string) {
    this.searchQuery = query.trim().toLowerCase();
    if (this.clearSearchBtn) {
      if (this.searchQuery.length > 0) {
        this.clearSearchBtn.classList.remove("hidden");
      } else {
        this.clearSearchBtn.classList.add("hidden");
      }
    }
    this.filterAndRender();
  }

  public renderMounts(mounts: MountItem[]) {
    this.mountListEl.innerHTML = "";
    if (this.mountCountEl) {
      this.mountCountEl.textContent = mounts.length.toString();
    }

    if (mounts.length === 0) {
      this.mountListEl.innerHTML = "<li class='loading'>No mounts configured</li>";
      return;
    }

    mounts.forEach((m, idx) => {
      const li = document.createElement("li");
      li.className = `mount-item ${idx === 0 ? "active" : ""}`;

      const titleSpan = document.createElement("span");
      titleSpan.textContent = m.id;

      li.appendChild(titleSpan);

      if (m.read_only) {
        const roBadge = document.createElement("span");
        roBadge.className = "drive-badge";
        roBadge.textContent = "RO";
        roBadge.title = "Read-Only Mount";
        li.appendChild(roBadge);
      }

      li.title = m.description || m.path_display;
      li.onclick = () => {
        document.querySelectorAll(".mount-item").forEach((el) => el.classList.remove("active"));
        li.classList.add("active");
        this.loadDirectory(m.id, "");
      };
      this.mountListEl.appendChild(li);
    });

    if (mounts.length > 0 && !this.currentMount) {
      this.loadDirectory(mounts[0].id, "");
    }
  }

  public renderDrives(drives: DiscoveredDriveItem[]) {
    if (!this.driveListEl) return;
    this.driveListEl.innerHTML = "";

    if (this.driveCountEl) {
      this.driveCountEl.textContent = drives.length.toString();
    }

    if (drives.length === 0) {
      this.driveListEl.innerHTML = "<li class='loading'>No physical drives detected</li>";
      return;
    }

    drives.forEach((d) => {
      const li = document.createElement("li");
      li.className = "drive-item";

      const nameDiv = document.createElement("div");
      nameDiv.className = "drive-name";
      nameDiv.innerHTML = `<span>&#128452;</span> <span>${d.name || d.path}</span>`;

      const badge = document.createElement("span");
      badge.className = `drive-badge ${d.is_system ? "system" : d.is_removable ? "removable" : ""}`;
      badge.textContent = d.is_system ? "System" : d.is_removable ? "USB" : d.drive_type || "Drive";

      li.appendChild(nameDiv);
      li.appendChild(badge);
      li.title = `Path: ${d.path} (${d.drive_type})`;
      this.driveListEl!.appendChild(li);
    });
  }

  async loadDirectory(mount: string, path = "") {
    this.currentMount = mount;
    this.currentPath = path;
    this.updateBreadcrumb();

    const loadingHtml = "<div class='loading'>Loading directory contents...</div>";
    this.fileGridEl.innerHTML = loadingHtml;
    if (this.fileListEl) {
      this.fileListEl.innerHTML = loadingHtml;
    }

    try {
      const res = await AetherApi.browse(mount, path);
      this.currentItems = res.items || [];
      this.filterAndRender();
    } catch (err: any) {
      const errHtml = `<div class='empty-state' style='color:var(--danger)'>${err.message || "Failed to load directory"}</div>`;
      this.fileGridEl.innerHTML = errHtml;
      if (this.fileListEl) {
        this.fileListEl.innerHTML = errHtml;
      }
    }
  }

  private filterAndRender() {
    let items = this.currentItems;
    if (this.searchQuery) {
      items = items.filter((item) => item.name.toLowerCase().includes(this.searchQuery));
    }

    if (items.length === 0) {
      const msg = this.searchQuery
        ? `No files matching "${this.searchQuery}" in this directory.`
        : "This directory is empty.";
      const emptyHtml = `<div class='empty-state'>${msg}</div>`;
      this.fileGridEl.innerHTML = emptyHtml;
      if (this.fileListEl) {
        this.fileListEl.innerHTML = emptyHtml;
      }
      return;
    }

    this.renderGrid(items);
    this.renderList(items);
  }

  private updateBreadcrumb() {
    const parts = this.currentPath.split("/").filter(Boolean);
    this.breadcrumbEl.innerHTML = "";

    const rootSpan = document.createElement("span");
    rootSpan.className = "breadcrumb-link";
    rootSpan.textContent = `[${this.currentMount || "Mounts"}]`;
    rootSpan.style.cursor = "pointer";
    rootSpan.onclick = () => {
      if (this.currentMount) {
        this.loadDirectory(this.currentMount, "");
      }
    };
    this.breadcrumbEl.appendChild(rootSpan);

    const rootSlash = document.createElement("span");
    rootSlash.textContent = " / ";
    this.breadcrumbEl.appendChild(rootSlash);

    let accum = "";
    parts.forEach((part, i) => {
      accum += (i === 0 ? "" : "/") + part;
      const target = accum;

      const seg = document.createElement("span");
      seg.className = "breadcrumb-link";
      seg.textContent = part;
      seg.style.cursor = "pointer";
      seg.onclick = () => this.loadDirectory(this.currentMount!, target);
      this.breadcrumbEl.appendChild(seg);

      if (i < parts.length - 1) {
        const slash = document.createElement("span");
        slash.textContent = " / ";
        this.breadcrumbEl.appendChild(slash);
      }
    });
  }

  private renderGrid(items: FileItem[]) {
    this.fileGridEl.innerHTML = "";

    items.forEach((item) => {
      const card = document.createElement("div");
      card.className = "file-card";

      const icon = document.createElement("div");
      icon.className = "file-icon";
      icon.innerHTML = item.is_dir ? "&#128193;" : this.getMediaIcon(item.name);

      const name = document.createElement("div");
      name.className = "file-name";
      name.textContent = item.name;
      name.title = item.name;

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

  private renderList(items: FileItem[]) {
    if (!this.fileListEl) return;
    this.fileListEl.innerHTML = "";

    const header = document.createElement("div");
    header.className = "file-list-header";
    header.innerHTML = `
      <span></span>
      <span>Name</span>
      <span>Modified</span>
      <span>Size</span>
      <span style="text-align:right">Actions</span>
    `;
    this.fileListEl.appendChild(header);

    items.forEach((item) => {
      const row = document.createElement("div");
      row.className = "file-list-row";

      const icon = document.createElement("div");
      icon.className = "file-list-icon";
      icon.innerHTML = item.is_dir ? "&#128193;" : this.getMediaIcon(item.name);

      const name = document.createElement("div");
      name.className = "file-list-name";
      name.textContent = item.name;
      name.title = item.name;

      const date = document.createElement("div");
      date.className = "file-list-date";
      date.textContent = this.formatDate(item.modified_utc);

      const size = document.createElement("div");
      size.className = "file-list-size";
      size.textContent = item.is_dir ? "Folder" : this.formatBytes(item.size);

      const actions = document.createElement("div");
      actions.className = "file-list-actions";

      if (item.is_dir) {
        row.onclick = () => {
          const next = this.currentPath ? `${this.currentPath}/${item.name}` : item.name;
          this.loadDirectory(this.currentMount!, next);
        };
      } else {
        const isVideo = this.isVideoFile(item.name);
        if (isVideo) {
          const playBtn = document.createElement("button");
          playBtn.className = "btn btn-primary btn-sm";
          playBtn.textContent = "Direct";
          playBtn.onclick = (e) => {
            e.stopPropagation();
            const filePath = this.currentPath ? `${this.currentPath}/${item.name}` : item.name;
            this.onPlayMedia?.(this.currentMount!, filePath, false);
          };
          actions.appendChild(playBtn);

          const hlsBtn = document.createElement("button");
          hlsBtn.className = "btn btn-secondary btn-sm";
          hlsBtn.textContent = "HLS";
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

      row.appendChild(icon);
      row.appendChild(name);
      row.appendChild(date);
      row.appendChild(size);
      row.appendChild(actions);

      this.fileListEl!.appendChild(row);
    });
  }

  private isVideoFile(fileName: string): boolean {
    return /\.(mp4|mkv|webm|mov|m4v|avi)$/i.test(fileName);
  }

  private getMediaIcon(fileName: string): string {
    if (this.isVideoFile(fileName)) return "&#127916;"; // film
    if (/\.(mp3|flac|wav|aac|ogg|m4a|opus)$/i.test(fileName)) return "&#127925;"; // audio
    if (/\.(jpg|jpeg|png|gif|webp|svg|avif)$/i.test(fileName)) return "&#128444;"; // image
    if (/\.(zip|tar|gz|7z|rar|bz2)$/i.test(fileName)) return "&#128230;"; // archive
    if (/\.(rs|ts|js|py|json|toml|yaml|yml|html|css|md)$/i.test(fileName)) return "&#128187;"; // code
    return "&#128196;"; // document
  }

  private formatBytes(bytes: number): string {
    if (bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB", "TB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
  }

  private formatDate(dateStr: string | null): string {
    if (!dateStr) return "--";
    try {
      const d = new Date(dateStr);
      if (isNaN(d.getTime())) return dateStr;
      return d.toLocaleDateString(undefined, {
        year: "numeric",
        month: "short",
        day: "numeric",
      });
    } catch {
      return dateStr;
    }
  }
}
