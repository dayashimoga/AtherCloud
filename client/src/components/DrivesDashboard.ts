import { VolumeInfoItem } from "../services/api";

export type DriveFilterCategory = "all" | "system" | "ssd" | "removable" | "read-only";

export class DrivesDashboardComponent {
  private containerEl: HTMLElement;
  private drives: VolumeInfoItem[] = [];
  private searchQuery = "";
  private activeFilter: DriveFilterCategory = "all";
  private selectedMountPoint: string | null = null;
  private isRefreshing = false;

  public onCreateMount: ((drive: VolumeInfoItem) => void) | null = null;
  public onRefresh: (() => Promise<void>) | null = null;
  public onExploreDrive: ((drive: VolumeInfoItem) => void) | null = null;

  constructor(containerId = "drives-view") {
    let el = document.getElementById(containerId);
    if (!el) {
      el = document.createElement("div");
      el.id = containerId;
      el.className = "view-panel hidden";
      const mainPanel = document.querySelector(".content-panel");
      if (mainPanel) {
        mainPanel.appendChild(el);
      } else {
        document.body.appendChild(el);
      }
    }
    this.containerEl = el;
    this.render();
  }

  public setDrives(drives: VolumeInfoItem[]) {
    this.drives = drives;
    this.render();
  }

  public selectDrive(mountPoint: string) {
    this.selectedMountPoint = mountPoint;
    this.render();
    const targetCard = this.containerEl.querySelector(`[data-mount-point="${CSS.escape(mountPoint)}"]`);
    if (targetCard) {
      targetCard.scrollIntoView({ behavior: "smooth", block: "center" });
      targetCard.classList.add("highlight-pulse");
      setTimeout(() => targetCard.classList.remove("highlight-pulse"), 2500);
    }
  }

  public getDrives(): VolumeInfoItem[] {
    return this.drives;
  }

  // --- Formatting Helpers ---

  public static formatBytes(bytes: number, decimals = 1): string {
    if (!bytes || bytes <= 0 || isNaN(bytes)) return "0 B";
    const k = 1024;
    const dm = decimals < 0 ? 0 : decimals;
    const sizes = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    const val = parseFloat((bytes / Math.pow(k, i)).toFixed(dm));
    return `${val} ${sizes[i] || "B"}`;
  }

  public static formatPercent(used: number, total: number): number {
    if (!total || total <= 0) return 0;
    const pct = (used / total) * 100;
    return Math.min(100, Math.max(0, parseFloat(pct.toFixed(1))));
  }

  public static getCapacityStatus(pct: number, isReadOnly: boolean): {
    color: string;
    label: string;
    cssClass: string;
  } {
    if (isReadOnly) {
      return { color: "var(--accent, #0ea5e9)", label: "Read-Only", cssClass: "status-readonly" };
    }
    if (pct >= 90) {
      return { color: "var(--danger, #ef4444)", label: "Critical Space", cssClass: "status-critical" };
    }
    if (pct >= 75) {
      return { color: "var(--warning, #f59e0b)", label: "Low Space", cssClass: "status-warning" };
    }
    return { color: "var(--success, #10b981)", label: "Healthy", cssClass: "status-healthy" };
  }

  public static getMediaIcon(mediaType: string): string {
    const mt = (mediaType || "").toLowerCase();
    if (mt.includes("nvme")) return "&#9889;"; // High-speed Lightning Bolt for NVMe
    if (mt.includes("ssd")) return "&#128190;"; // Floppy/SSD chip symbol
    if (mt.includes("rotational") || mt.includes("hdd")) return "&#128191;"; // Optical/HDD magnetic disk
    if (mt.includes("removable") || mt.includes("usb")) return "&#128268;"; // USB Plug
    if (mt.includes("optical")) return "&#128191;";
    if (mt.includes("network")) return "&#127760;"; // Globe
    return "&#128452;"; // Server/Drive cabinet
  }

  public static getMediaDisplayName(mediaType: string): string {
    switch (mediaType) {
      case "NvmeSsd":
        return "NVMe M.2 Solid State Drive";
      case "SataSsd":
        return "SATA Solid State Drive";
      case "RotationalHdd":
        return "Rotational Hard Disk Drive";
      case "RemovableUsb":
        return "Removable USB Flash Drive";
      case "OpticalDrive":
        return "Optical Disc Drive";
      case "NetworkDrive":
        return "Network Attached Storage (NAS)";
      default:
        return mediaType || "Physical Storage Volume";
    }
  }

  // --- Filtering Logic ---

  private getFilteredDrives(): VolumeInfoItem[] {
    const q = this.searchQuery.trim().toLowerCase();
    return this.drives.filter((d) => {
      // Category filter
      if (this.activeFilter === "system" && !d.is_system) return false;
      if (this.activeFilter === "ssd" && !d.media_type.toLowerCase().includes("ssd")) return false;
      if (this.activeFilter === "removable" && !d.is_removable) return false;
      if (this.activeFilter === "read-only" && !d.is_read_only) return false;

      // Text query filter
      if (q) {
        const nameMatch = (d.name || "").toLowerCase().includes(q);
        const mountMatch = (d.mount_point || "").toLowerCase().includes(q);
        const fsMatch = (d.filesystem || "").toLowerCase().includes(q);
        const labelMatch = (d.filesystem_label || "").toLowerCase().includes(q);
        const mediaMatch = (d.media_type || "").toLowerCase().includes(q);
        const serialMatch = (d.serial_number || "").toLowerCase().includes(q);
        if (!nameMatch && !mountMatch && !fsMatch && !labelMatch && !mediaMatch && !serialMatch) {
          return false;
        }
      }
      return true;
    });
  }

  // --- Primary Render ---

  public render() {
    let totalHostBytes = 0;
    let totalAvailBytes = 0;
    let healthyCount = 0;

    for (const d of this.drives) {
      totalHostBytes += d.total_bytes || 0;
      totalAvailBytes += d.available_bytes || 0;
      const used = (d.total_bytes || 0) - (d.available_bytes || 0);
      const pct = DrivesDashboardComponent.formatPercent(used, d.total_bytes);
      if (pct < 90 && !d.is_read_only) healthyCount++;
    }

    const totalUsedBytes = Math.max(0, totalHostBytes - totalAvailBytes);
    const overallUsedPct = DrivesDashboardComponent.formatPercent(totalUsedBytes, totalHostBytes);
    const filtered = this.getFilteredDrives();

    this.containerEl.innerHTML = `
      <div class="drives-dashboard-container">
        <!-- Dashboard Header & Telemetry Summary Banner -->
        <div class="drives-header-row">
          <div class="drives-title-area">
            <div class="drives-title-badge">
              <span class="drives-badge-icon">&#128452;</span>
              <h2>Host Storage & Disks Dashboard</h2>
            </div>
            <p class="drives-subtitle">Real-time physical disk telemetry, partition capacity allocation, filesystem diagnostics & virtual mount discovery</p>
          </div>
          <div class="drives-header-actions">
            <button id="refresh-drives-btn" class="btn btn-secondary btn-sm ${this.isRefreshing ? "spinning" : ""}" title="Rescan physical host storage disks">
              <span class="btn-icon">&#8635;</span> Rescan Disks
            </button>
          </div>
        </div>

        <!-- Global Storage Capacity KPI Cards -->
        <div class="drives-summary-cards">
          <div class="summary-metric-card">
            <div class="metric-icon total">&#128190;</div>
            <div class="metric-body">
              <span class="metric-label">Total Storage Capacity</span>
              <span class="metric-value">${DrivesDashboardComponent.formatBytes(totalHostBytes)}</span>
              <span class="metric-subtext">Across ${this.drives.length} physical volumes</span>
            </div>
          </div>

          <div class="summary-metric-card">
            <div class="metric-icon used">&#128200;</div>
            <div class="metric-body">
              <span class="metric-label">Used Space (${overallUsedPct}%)</span>
              <span class="metric-value">${DrivesDashboardComponent.formatBytes(totalUsedBytes)}</span>
              <div class="summary-mini-track">
                <div class="summary-mini-fill" style="width: ${overallUsedPct}%; background: ${DrivesDashboardComponent.getCapacityStatus(overallUsedPct, false).color};"></div>
              </div>
            </div>
          </div>

          <div class="summary-metric-card">
            <div class="metric-icon free">&#128994;</div>
            <div class="metric-body">
              <span class="metric-label">Available Free Space</span>
              <span class="metric-value">${DrivesDashboardComponent.formatBytes(totalAvailBytes)}</span>
              <span class="metric-subtext">${(100 - overallUsedPct).toFixed(1)}% remaining headroom</span>
            </div>
          </div>

          <div class="summary-metric-card">
            <div class="metric-icon disks">&#9881;</div>
            <div class="metric-body">
              <span class="metric-label">Disks Health Status</span>
              <span class="metric-value">${healthyCount} / ${this.drives.length} Healthy</span>
              <span class="metric-subtext">${healthyCount === this.drives.length ? "All volumes operational" : "Check capacity alerts"}</span>
            </div>
          </div>
        </div>

        <!-- Filter & Search Toolbar -->
        <div class="drives-filter-toolbar">
          <div class="drives-search-box">
            <span class="search-icon">&#128269;</span>
            <input 
              type="text" 
              id="drives-search-input" 
              placeholder="Search by drive name, mount point, filesystem label, or serial..." 
              value="${this.searchQuery}" 
              autocomplete="off" 
            />
            ${this.searchQuery ? `<button id="clear-drives-search" class="btn-clear" title="Clear search">&times;</button>` : ""}
          </div>

          <div class="drives-filter-pills">
            <button class="filter-pill ${this.activeFilter === "all" ? "active" : ""}" data-filter="all">All Disks (${this.drives.length})</button>
            <button class="filter-pill ${this.activeFilter === "system" ? "active" : ""}" data-filter="system">System Boot (${this.drives.filter((d) => d.is_system).length})</button>
            <button class="filter-pill ${this.activeFilter === "ssd" ? "active" : ""}" data-filter="ssd">SSDs / NVMe (${this.drives.filter((d) => d.media_type.toLowerCase().includes("ssd")).length})</button>
            <button class="filter-pill ${this.activeFilter === "removable" ? "active" : ""}" data-filter="removable">Removable USB (${this.drives.filter((d) => d.is_removable).length})</button>
            <button class="filter-pill ${this.activeFilter === "read-only" ? "active" : ""}" data-filter="read-only">Read-Only (${this.drives.filter((d) => d.is_read_only).length})</button>
          </div>
        </div>

        <!-- Disks Grid List -->
        <div class="drives-grid" id="drives-grid-list">
          ${this.renderDrivesGrid(filtered)}
        </div>
      </div>
    `;

    this.bindEvents();
  }

  private renderDrivesGrid(drives: VolumeInfoItem[]): string {
    if (this.drives.length === 0) {
      return `
        <div class="drives-empty-state">
          <div class="empty-icon">&#128452;</div>
          <h3>Scanning Host Storage Volumes...</h3>
          <p>Contacting storage telemetry daemon to enumerate physical drives and partitions.</p>
          <button id="empty-rescan-btn" class="btn btn-primary btn-sm" style="margin-top: 12px;">&#8635; Retry Volume Scan</button>
        </div>
      `;
    }

    if (drives.length === 0) {
      return `
        <div class="drives-empty-state">
          <div class="empty-icon">&#128269;</div>
          <h3>No Disks Match Filter</h3>
          <p>No storage volumes found matching category <strong>"${this.activeFilter}"</strong> and query <strong>"${this.searchQuery}"</strong>.</p>
          <button id="reset-filter-btn" class="btn btn-secondary btn-sm" style="margin-top: 12px;">Clear Filters</button>
        </div>
      `;
    }

    return drives.map((d) => this.renderDriveCard(d)).join("");
  }

  private renderDriveCard(drive: VolumeInfoItem): string {
    const totalBytes = drive.total_bytes || 0;
    const availBytes = drive.available_bytes || 0;
    const usedBytes = Math.max(0, totalBytes - availBytes);
    const usedPct = DrivesDashboardComponent.formatPercent(usedBytes, totalBytes);
    const status = DrivesDashboardComponent.getCapacityStatus(usedPct, drive.is_read_only);
    const isSelected = this.selectedMountPoint === drive.mount_point;
    const icon = DrivesDashboardComponent.getMediaIcon(drive.media_type);
    const mediaName = DrivesDashboardComponent.getMediaDisplayName(drive.media_type);

    return `
      <div class="drive-card ${isSelected ? "selected" : ""}" data-mount-point="${drive.mount_point}">
        <!-- Drive Card Header -->
        <div class="drive-card-header">
          <div class="drive-card-identity">
            <span class="drive-card-icon" title="${mediaName}">${icon}</span>
            <div class="drive-card-titles">
              <div class="drive-card-name-row">
                <span class="drive-card-name">${drive.name || drive.mount_point}</span>
                <span class="drive-status-pill ${status.cssClass}">
                  <span class="status-dot"></span> ${status.label}
                </span>
              </div>
              <div class="drive-card-mount-row">
                <code class="drive-mount-path" title="Physical Mount Point">${drive.mount_point}</code>
                <button class="btn-copy-path" data-path="${drive.mount_point}" title="Copy mount point to clipboard">&#128203;</button>
              </div>
            </div>
          </div>
          <div class="drive-card-badges">
            <span class="badge-fs" title="Filesystem">${drive.filesystem || "Unknown FS"}</span>
            ${drive.filesystem_label ? `<span class="badge-label" title="Filesystem Volume Label">${drive.filesystem_label}</span>` : ""}
            ${drive.is_system ? `<span class="badge-system" title="Host Operating System Boot Partition">&#9733; OS Boot</span>` : ""}
            ${drive.is_removable ? `<span class="badge-removable" title="Removable Hot-Plug Media">&#128268; USB / Hot-Plug</span>` : ""}
            ${drive.is_read_only ? `<span class="badge-ro" title="Write-Protected Read-Only">&#128274; Read-Only</span>` : ""}
          </div>
        </div>

        <!-- Capacity Progress Bar Widget -->
        <div class="drive-card-capacity-widget">
          <div class="capacity-stats-row">
            <span class="capacity-used">
              <strong>${DrivesDashboardComponent.formatBytes(usedBytes)}</strong> Used (${usedPct}%)
            </span>
            <span class="capacity-free">
              <strong>${DrivesDashboardComponent.formatBytes(availBytes)}</strong> Free of ${DrivesDashboardComponent.formatBytes(totalBytes)}
            </span>
          </div>
          <div class="drive-capacity-track">
            <div 
              class="drive-capacity-bar" 
              style="width: ${usedPct}%; background: ${status.color};"
              title="Storage Utilization: ${usedPct}% (${DrivesDashboardComponent.formatBytes(usedBytes)} / ${DrivesDashboardComponent.formatBytes(totalBytes)})"
            ></div>
          </div>
        </div>

        <!-- Telemetry Details Grid -->
        <div class="drive-telemetry-grid">
          <div class="telemetry-cell">
            <span class="telemetry-key">Media Technology</span>
            <span class="telemetry-val">${mediaName}</span>
          </div>
          <div class="telemetry-cell">
            <span class="telemetry-key">Filesystem Type</span>
            <span class="telemetry-val">${drive.filesystem} ${drive.filesystem_label ? `("${drive.filesystem_label}")` : ""}</span>
          </div>
          <div class="telemetry-cell">
            <span class="telemetry-key">Access Permissions</span>
            <span class="telemetry-val">${drive.is_read_only ? "Read-Only (ro)" : "Read-Write (rw)"}</span>
          </div>
          <div class="telemetry-cell">
            <span class="telemetry-key">Serial / Hardware ID</span>
            <span class="telemetry-val" title="${drive.serial_number || "Hardware serial not exported by host OS"}">
              ${drive.serial_number ? drive.serial_number : "<span class='text-muted'>Standard Host Device</span>"}
            </span>
          </div>
        </div>

        <!-- Card Action Footer -->
        <div class="drive-card-footer">
          <button class="btn btn-primary btn-sm btn-mount-drive" data-mount-point="${drive.mount_point}">
            <span class="btn-icon">+</span> Create Virtual Mount
          </button>
          <button class="btn btn-secondary btn-sm btn-copy-path-full" data-path="${drive.mount_point}">
            <span class="btn-icon">&#128203;</span> Copy Path
          </button>
        </div>
      </div>
    `;
  }

  // --- DOM Event Bindings ---

  private bindEvents() {
    // Refresh button
    const refreshBtn = this.containerEl.querySelector("#refresh-drives-btn");
    if (refreshBtn) {
      refreshBtn.addEventListener("click", async () => {
        this.isRefreshing = true;
        this.render();
        try {
          if (this.onRefresh) await this.onRefresh();
        } finally {
          this.isRefreshing = false;
          this.render();
        }
      });
    }

    // Empty state retry
    const emptyBtn = this.containerEl.querySelector("#empty-rescan-btn");
    if (emptyBtn) {
      emptyBtn.addEventListener("click", () => {
        if (this.onRefresh) this.onRefresh();
      });
    }

    // Reset filter button
    const resetBtn = this.containerEl.querySelector("#reset-filter-btn");
    if (resetBtn) {
      resetBtn.addEventListener("click", () => {
        this.searchQuery = "";
        this.activeFilter = "all";
        this.render();
      });
    }

    // Search input
    const searchInput = this.containerEl.querySelector("#drives-search-input") as HTMLInputElement | null;
    if (searchInput) {
      searchInput.addEventListener("input", (e) => {
        this.searchQuery = (e.target as HTMLInputElement).value;
        this.updateGridOnly();
      });
    }

    // Clear search button
    const clearSearchBtn = this.containerEl.querySelector("#clear-drives-search");
    if (clearSearchBtn) {
      clearSearchBtn.addEventListener("click", () => {
        this.searchQuery = "";
        this.render();
      });
    }

    // Filter pills
    const pills = this.containerEl.querySelectorAll(".filter-pill");
    pills.forEach((p) => {
      p.addEventListener("click", () => {
        this.activeFilter = p.getAttribute("data-filter") as DriveFilterCategory;
        this.render();
      });
    });

    // Mount drive buttons
    const mountBtns = this.containerEl.querySelectorAll(".btn-mount-drive");
    mountBtns.forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        const mp = btn.getAttribute("data-mount-point");
        const drive = this.drives.find((d) => d.mount_point === mp);
        if (drive && this.onCreateMount) {
          this.onCreateMount(drive);
        }
      });
    });

    // Copy path buttons
    const copyBtns = this.containerEl.querySelectorAll(".btn-copy-path, .btn-copy-path-full");
    copyBtns.forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        const p = btn.getAttribute("data-path");
        if (p) {
          navigator.clipboard.writeText(p).then(() => {
            const originalText = btn.innerHTML;
            btn.innerHTML = "&#10003; Copied!";
            setTimeout(() => {
              btn.innerHTML = originalText;
            }, 1800);
          });
        }
      });
    });

    // Card selection
    const cards = this.containerEl.querySelectorAll(".drive-card");
    cards.forEach((card) => {
      card.addEventListener("click", () => {
        const mp = card.getAttribute("data-mount-point");
        if (mp) {
          this.selectedMountPoint = mp;
          cards.forEach((c) => c.classList.remove("selected"));
          card.classList.add("selected");
        }
      });
    });
  }

  private updateGridOnly() {
    const gridEl = this.containerEl.querySelector("#drives-grid-list");
    if (gridEl) {
      const filtered = this.getFilteredDrives();
      gridEl.innerHTML = this.renderDrivesGrid(filtered);
      this.bindCardEvents();
    }
  }

  private bindCardEvents() {
    const mountBtns = this.containerEl.querySelectorAll(".btn-mount-drive");
    mountBtns.forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        const mp = btn.getAttribute("data-mount-point");
        const drive = this.drives.find((d) => d.mount_point === mp);
        if (drive && this.onCreateMount) {
          this.onCreateMount(drive);
        }
      });
    });

    const copyBtns = this.containerEl.querySelectorAll(".btn-copy-path, .btn-copy-path-full");
    copyBtns.forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        const p = btn.getAttribute("data-path");
        if (p) {
          navigator.clipboard.writeText(p).then(() => {
            const originalText = btn.innerHTML;
            btn.innerHTML = "&#10003; Copied!";
            setTimeout(() => {
              btn.innerHTML = originalText;
            }, 1800);
          });
        }
      });
    });

    const cards = this.containerEl.querySelectorAll(".drive-card");
    cards.forEach((card) => {
      card.addEventListener("click", () => {
        const mp = card.getAttribute("data-mount-point");
        if (mp) {
          this.selectedMountPoint = mp;
          cards.forEach((c) => c.classList.remove("selected"));
          card.classList.add("selected");
        }
      });
    });
  }
}
