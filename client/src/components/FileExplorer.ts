import { AetherApi, DiscoveredDriveItem, FileItem, MountItem } from "../services/api";
import { ShareService } from "../services/shareService";

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

  // File Operations: Multi-Select, Drag-Drop, and Chunked Uploads
  private selectionBarEl: HTMLElement | null = null;
  private selectionCountEl: HTMLElement | null = null;
  private selectAllBtn: HTMLButtonElement | null = null;
  private downloadSelectedBtn: HTMLButtonElement | null = null;
  private clearSelectionBtn: HTMLButtonElement | null = null;
  private uploadBtn: HTMLButtonElement | null = null;
  private fileUploadInput: HTMLInputElement | null = null;
  private fileContainerEl: HTMLElement | null = null;
  private dragOverlayEl: HTMLElement | null = null;

  // Upload Progress Toast
  private uploadProgressModalEl: HTMLElement | null = null;
  private uploadFilenameEl: HTMLElement | null = null;
  private uploadPercentEl: HTMLElement | null = null;
  private uploadProgressBarEl: HTMLElement | null = null;
  private uploadChunkInfoEl: HTMLElement | null = null;
  private uploadStatusEl: HTMLElement | null = null;

  private currentMount: string | null = null;
  private currentPath = "";
  private currentItems: FileItem[] = [];
  private searchQuery = "";
  private viewMode: ViewMode = "grid";
  private selectedFiles: Set<string> = new Set();

  public onPlayMedia: ((mount: string, path: string, isHls: boolean) => void) | null = null;
  public onShareItem: ((mount: string, path: string) => void) | null = null;
  public onSelectDrive: ((drive: DiscoveredDriveItem) => void) | null = null;
  public onSelectMount: ((mount: MountItem) => void) | null = null;

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

    this.selectionBarEl = document.getElementById("selection-bar");
    this.selectionCountEl = document.getElementById("selection-count");
    this.selectAllBtn = document.getElementById("select-all-btn") as HTMLButtonElement | null;
    this.downloadSelectedBtn = document.getElementById("download-selected-btn") as HTMLButtonElement | null;
    this.clearSelectionBtn = document.getElementById("clear-selection-btn") as HTMLButtonElement | null;
    this.uploadBtn = document.getElementById("upload-btn") as HTMLButtonElement | null;
    this.fileUploadInput = document.getElementById("file-upload-input") as HTMLInputElement | null;
    this.fileContainerEl = document.getElementById("file-container");
    this.dragOverlayEl = document.getElementById("drag-overlay");

    this.uploadProgressModalEl = document.getElementById("upload-progress-modal");
    this.uploadFilenameEl = document.getElementById("upload-filename");
    this.uploadPercentEl = document.getElementById("upload-percent");
    this.uploadProgressBarEl = document.getElementById("upload-progress-bar");
    this.uploadChunkInfoEl = document.getElementById("upload-chunk-info");
    this.uploadStatusEl = document.getElementById("upload-status");

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

    // Multi-Select Actions
    if (this.selectAllBtn) {
      this.selectAllBtn.addEventListener("click", () => this.selectAllFiles());
    }
    if (this.clearSelectionBtn) {
      this.clearSelectionBtn.addEventListener("click", () => this.clearSelection());
    }
    if (this.downloadSelectedBtn) {
      this.downloadSelectedBtn.addEventListener("click", () => this.downloadSelectedFiles());
    }

    // Upload Triggers
    if (this.uploadBtn && this.fileUploadInput) {
      this.uploadBtn.addEventListener("click", () => {
        this.fileUploadInput?.click();
      });
      this.fileUploadInput.addEventListener("change", () => {
        if (this.fileUploadInput?.files && this.fileUploadInput.files.length > 0) {
          this.handleUploadFiles(this.fileUploadInput.files);
          this.fileUploadInput.value = "";
        }
      });
    }

    // Drag and Drop Uploads
    if (this.fileContainerEl) {
      ["dragenter", "dragover"].forEach((eventName) => {
        this.fileContainerEl!.addEventListener(eventName, (e) => {
          e.preventDefault();
          e.stopPropagation();
          this.dragOverlayEl?.classList.remove("hidden");
        });
      });

      ["dragleave", "dragend"].forEach((eventName) => {
        this.fileContainerEl!.addEventListener(eventName, (e) => {
          e.preventDefault();
          e.stopPropagation();
          this.dragOverlayEl?.classList.add("hidden");
        });
      });

      this.fileContainerEl.addEventListener("drop", (e: DragEvent) => {
        e.preventDefault();
        e.stopPropagation();
        this.dragOverlayEl?.classList.add("hidden");
        if (e.dataTransfer?.files && e.dataTransfer.files.length > 0) {
          this.handleUploadFiles(e.dataTransfer.files);
        }
      });
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
        document.querySelectorAll(".drive-item").forEach((el) => el.classList.remove("active"));
        li.classList.add("active");
        this.loadDirectory(m.id, "");
        if (this.onSelectMount) {
          this.onSelectMount(m);
        }
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
      li.className = "drive-item clickable";

      const total = d.total_bytes || 0;
      const avail = d.available_bytes || 0;
      const used = Math.max(0, total - avail);
      const pct = total > 0 ? Math.min(100, Math.max(0, Math.round((used / total) * 100))) : 0;
      const color = d.is_read_only
        ? "var(--accent, #0ea5e9)"
        : pct >= 90
        ? "var(--danger, #ef4444)"
        : pct >= 75
        ? "var(--warning, #f59e0b)"
        : "var(--success, #10b981)";

      const mt = (d.media_type || d.drive_type || "").toLowerCase();
      const icon = mt.includes("nvme")
        ? "&#9889;"
        : mt.includes("ssd")
        ? "&#128190;"
        : mt.includes("removable") || mt.includes("usb")
        ? "&#128268;"
        : "&#128452;";

      li.innerHTML = `
        <div class="drive-item-main">
          <div class="drive-name">
            <span class="drive-icon">${icon}</span>
            <span class="drive-title" title="${d.mount_point || d.path}">${d.name || d.mount_point || d.path}</span>
          </div>
          <div class="drive-badges-row">
            ${d.filesystem ? `<span class="drive-fs-pill">${d.filesystem}</span>` : ""}
            <span class="drive-badge ${d.is_system ? "system" : d.is_removable ? "removable" : d.is_read_only ? "readonly" : ""}">
              ${d.is_system ? "System" : d.is_removable ? "USB" : d.is_read_only ? "RO" : d.drive_type || "Drive"}
            </span>
          </div>
        </div>
        ${total > 0 ? `
          <div class="sidebar-drive-capacity">
            <div class="sidebar-drive-track">
              <div class="sidebar-drive-bar" style="width: ${pct}%; background: ${color};"></div>
            </div>
            <div class="sidebar-drive-metric">
              <span>${pct}% used</span>
              <span>${d.filesystem_label || d.filesystem || ""}</span>
            </div>
          </div>
        ` : ""}
      `;

      li.title = `Path: ${d.mount_point || d.path} (${d.drive_type || d.media_type})\nFilesystem: ${d.filesystem}\nClick to inspect in Storage & Drives Dashboard`;
      li.onclick = () => {
        document.querySelectorAll(".mount-item").forEach((el) => el.classList.remove("active"));
        document.querySelectorAll(".drive-item").forEach((el) => el.classList.remove("active"));
        li.classList.add("active");
        if (this.onSelectDrive) {
          this.onSelectDrive(d);
        }
      };

      this.driveListEl!.appendChild(li);
    });
  }

  async loadDirectory(mount: string, path = "") {
    this.currentMount = mount;
    this.currentPath = path;
    this.selectedFiles.clear();
    this.updateSelectionUI();
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

  private toggleSelectFile(fileName: string, isChecked?: boolean) {
    if (isChecked !== undefined) {
      if (isChecked) {
        this.selectedFiles.add(fileName);
      } else {
        this.selectedFiles.delete(fileName);
      }
    } else {
      if (this.selectedFiles.has(fileName)) {
        this.selectedFiles.delete(fileName);
      } else {
        this.selectedFiles.add(fileName);
      }
    }
    this.updateSelectionUI();
  }

  private selectAllFiles() {
    this.currentItems.forEach((item) => {
      if (!item.is_dir) {
        this.selectedFiles.add(item.name);
      }
    });
    this.updateSelectionUI();
    this.syncSelectionDom();
  }

  private clearSelection() {
    this.selectedFiles.clear();
    this.updateSelectionUI();
    this.syncSelectionDom();
  }

  private updateSelectionUI() {
    const count = this.selectedFiles.size;
    if (this.selectionBarEl) {
      if (count > 0) {
        this.selectionBarEl.classList.remove("hidden");
      } else {
        this.selectionBarEl.classList.add("hidden");
      }
    }
    if (this.selectionCountEl) {
      this.selectionCountEl.textContent = `${count} file${count === 1 ? "" : "s"} selected`;
    }
  }

  private syncSelectionDom() {
    document.querySelectorAll(".file-checkbox, .file-list-checkbox").forEach((cb) => {
      const input = cb as HTMLInputElement;
      input.checked = this.selectedFiles.has(input.dataset.filename || "");
    });
    document.querySelectorAll(".file-card").forEach((card) => {
      const name = card.querySelector(".file-name")?.textContent || "";
      if (this.selectedFiles.has(name)) {
        card.classList.add("selected");
      } else {
        card.classList.remove("selected");
      }
    });
    document.querySelectorAll(".file-list-row").forEach((row) => {
      const name = row.querySelector(".file-list-name")?.textContent || "";
      if (this.selectedFiles.has(name)) {
        row.classList.add("selected");
      } else {
        row.classList.remove("selected");
      }
    });
  }

  private downloadSelectedFiles() {
    if (this.selectedFiles.size === 0 || !this.currentMount) return;
    this.selectedFiles.forEach((fileName) => {
      const filePath = this.currentPath ? `${this.currentPath}/${fileName}` : fileName;
      this.downloadFile(this.currentMount!, filePath);
    });
  }

  public downloadFile(mount: string, path: string) {
    const url = AetherApi.getDownloadUrl(mount, path);
    const link = document.createElement("a");
    link.href = url;
    link.download = path.split("/").pop() || "download";
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  }

  async handleUploadFiles(files: FileList | File[]) {
    if (!files || files.length === 0 || !this.currentMount) return;

    if (this.uploadProgressModalEl) {
      this.uploadProgressModalEl.classList.remove("hidden");
    }

    const chunkSize = 2 * 1024 * 1024; // 2 MiB chunks

    for (let f = 0; f < files.length; f++) {
      const file = files[f];
      const targetPath = this.currentPath ? `${this.currentPath}/${file.name}` : file.name;

      if (this.uploadFilenameEl) {
        this.uploadFilenameEl.textContent = `${file.name} (${f + 1}/${files.length})`;
      }

      try {
        const init = await AetherApi.initUpload(this.currentMount, targetPath, file.size, chunkSize);
        const totalChunks = init.total_chunks;

        for (let i = 0; i < totalChunks; i++) {
          const start = i * chunkSize;
          const end = Math.min(start + chunkSize, file.size);
          const chunkBlob = file.slice(start, end);
          const chunkBuffer = await chunkBlob.arrayBuffer();
          const sha256 = await AetherApi.computeSha256(chunkBuffer);

          if (this.uploadChunkInfoEl) {
            this.uploadChunkInfoEl.textContent = `Chunk ${i + 1}/${totalChunks}`;
          }
          if (this.uploadStatusEl) {
            this.uploadStatusEl.textContent = `${Math.round((end / file.size) * 100)}% verified`;
          }

          await AetherApi.uploadChunk(this.currentMount, init.upload_id, i, sha256, chunkBuffer);

          const pct = Math.round(((i + 1) / totalChunks) * 100);
          if (this.uploadProgressBarEl) {
            this.uploadProgressBarEl.style.width = `${pct}%`;
          }
          if (this.uploadPercentEl) {
            this.uploadPercentEl.textContent = `${pct}%`;
          }
        }

        await AetherApi.completeUpload(this.currentMount, init.upload_id);
      } catch (err: any) {
        alert(`Upload error for ${file.name}: ${err.message}`);
      }
    }

    if (this.uploadProgressModalEl) {
      this.uploadProgressModalEl.classList.add("hidden");
    }

    await this.loadDirectory(this.currentMount, this.currentPath);
  }

  private renderGrid(items: FileItem[]) {
    this.fileGridEl.innerHTML = "";

    items.forEach((item) => {
      const card = document.createElement("div");
      card.className = `file-card ${this.selectedFiles.has(item.name) ? "selected" : ""}`;

      // Checkbox for non-directories
      if (!item.is_dir) {
        const cb = document.createElement("input");
        cb.type = "checkbox";
        cb.className = "file-checkbox";
        cb.dataset.filename = item.name;
        cb.checked = this.selectedFiles.has(item.name);
        cb.onclick = (e) => {
          e.stopPropagation();
          this.toggleSelectFile(item.name, cb.checked);
          if (cb.checked) {
            card.classList.add("selected");
          } else {
            card.classList.remove("selected");
          }
        };
        card.appendChild(cb);
      }

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
          playBtn.textContent = "Play";
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

        const downloadBtn = document.createElement("button");
        downloadBtn.className = "btn btn-secondary btn-sm";
        downloadBtn.innerHTML = "&#128229;";
        downloadBtn.title = "Direct Download";
        downloadBtn.onclick = (e) => {
          e.stopPropagation();
          const filePath = this.currentPath ? `${this.currentPath}/${item.name}` : item.name;
          this.downloadFile(this.currentMount!, filePath);
        };
        actions.appendChild(downloadBtn);

        const shareBtn = document.createElement("button");
        shareBtn.className = "btn btn-secondary btn-sm";
        shareBtn.textContent = "Share";
        shareBtn.onclick = (e) => {
          e.stopPropagation();
          const filePath = this.currentPath ? `${this.currentPath}/${item.name}` : item.name;
          this.onShareItem?.(this.currentMount!, filePath);
        };
        actions.appendChild(shareBtn);

        const quickShareBtn = document.createElement("button");
        quickShareBtn.className = "btn btn-secondary btn-sm btn-quick-share";
        quickShareBtn.title = "1-Tap Quick Share Link";
        quickShareBtn.innerHTML = "&#128279;";
        quickShareBtn.onclick = (e) => {
          e.stopPropagation();
          const filePath = this.currentPath ? `${this.currentPath}/${item.name}` : item.name;
          this.quickShareFile(this.currentMount!, filePath, item.name);
        };
        actions.appendChild(quickShareBtn);
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
      row.className = `file-list-row ${this.selectedFiles.has(item.name) ? "selected" : ""}`;

      const icon = document.createElement("div");
      icon.className = "file-list-icon";
      if (!item.is_dir) {
        const cb = document.createElement("input");
        cb.type = "checkbox";
        cb.className = "file-list-checkbox";
        cb.dataset.filename = item.name;
        cb.checked = this.selectedFiles.has(item.name);
        cb.onclick = (e) => {
          e.stopPropagation();
          this.toggleSelectFile(item.name, cb.checked);
          if (cb.checked) {
            row.classList.add("selected");
          } else {
            row.classList.remove("selected");
          }
        };
        icon.appendChild(cb);
      } else {
        icon.innerHTML = "&#128193;";
      }

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

        const downloadBtn = document.createElement("button");
        downloadBtn.className = "btn btn-secondary btn-sm";
        downloadBtn.innerHTML = "&#128229;";
        downloadBtn.title = "Direct Download";
        downloadBtn.onclick = (e) => {
          e.stopPropagation();
          const filePath = this.currentPath ? `${this.currentPath}/${item.name}` : item.name;
          this.downloadFile(this.currentMount!, filePath);
        };
        actions.appendChild(downloadBtn);

        const shareBtn = document.createElement("button");
        shareBtn.className = "btn btn-secondary btn-sm";
        shareBtn.textContent = "Share";
        shareBtn.onclick = (e) => {
          e.stopPropagation();
          const filePath = this.currentPath ? `${this.currentPath}/${item.name}` : item.name;
          this.onShareItem?.(this.currentMount!, filePath);
        };
        actions.appendChild(shareBtn);

        const quickShareBtn = document.createElement("button");
        quickShareBtn.className = "btn btn-secondary btn-sm btn-quick-share";
        quickShareBtn.title = "1-Tap Quick Share Link";
        quickShareBtn.innerHTML = "&#128279;";
        quickShareBtn.onclick = (e) => {
          e.stopPropagation();
          const filePath = this.currentPath ? `${this.currentPath}/${item.name}` : item.name;
          this.quickShareFile(this.currentMount!, filePath, item.name);
        };
        actions.appendChild(quickShareBtn);
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

  private async quickShareFile(mount: string, path: string, fileName: string) {
    const isVideo = this.isVideoFile(fileName);
    const streamUrl = isVideo
      ? AetherApi.getHlsMasterUrl(mount, path)
      : AetherApi.getStreamUrl(mount, path);
    const fullUrl = streamUrl.startsWith("http")
      ? streamUrl
      : `${window.location.origin}${streamUrl}`;

    await ShareService.getInstance().share({
      title: `AetherStream: ${fileName}`,
      text: `Stream or download ${fileName} from AetherStream`,
      url: fullUrl,
    });
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
