import { AetherApi, CreateMountPayload, VolumeInfoItem } from "../services/api";

export class CreateMountModalComponent {
  private modalEl: HTMLElement;
  private formEl: HTMLFormElement;
  private nameInput: HTMLInputElement;
  private pathInput: HTMLInputElement;
  private descInput: HTMLInputElement;
  private quickDrivesContainer: HTMLElement;
  private quarantineWarning: HTMLElement;
  private tagsContainer: HTMLElement;
  private tagInput: HTMLInputElement;
  private addTagBtn: HTMLButtonElement;
  private passwordInput: HTMLInputElement;
  private expirySelect: HTMLSelectElement;
  private submitBtn: HTMLButtonElement;
  private cancelBtn: HTMLButtonElement;
  private closeBtn: HTMLButtonElement;
  private errorBanner: HTMLElement;

  private allowedSubpaths: Set<string> = new Set();
  private availableDrives: VolumeInfoItem[] = [];
  private selectedPermMode: "read_only" | "read_write" | "stream_only" = "read_only";
  private isSubmitting = false;

  public onMountCreated: ((mount: any) => void) | null = null;

  constructor() {
    this.modalEl = document.getElementById("create-mount-modal")!;
    this.formEl = document.getElementById("create-mount-form") as HTMLFormElement;
    this.nameInput = document.getElementById("create-mount-name") as HTMLInputElement;
    this.pathInput = document.getElementById("create-mount-path") as HTMLInputElement;
    this.descInput = document.getElementById("create-mount-desc") as HTMLInputElement;
    this.quickDrivesContainer = document.getElementById("mount-quick-drives")!;
    this.quarantineWarning = document.getElementById("mount-quarantine-warning")!;
    this.tagsContainer = document.getElementById("subpath-tags-list")!;
    this.tagInput = document.getElementById("subpath-tag-input") as HTMLInputElement;
    this.addTagBtn = document.getElementById("add-subpath-tag-btn") as HTMLButtonElement;
    this.passwordInput = document.getElementById("create-mount-password") as HTMLInputElement;
    this.expirySelect = document.getElementById("create-mount-expiry") as HTMLSelectElement;
    this.submitBtn = document.getElementById("submit-create-mount-btn") as HTMLButtonElement;
    this.cancelBtn = document.getElementById("cancel-create-mount-btn") as HTMLButtonElement;
    this.closeBtn = document.getElementById("close-create-mount-btn") as HTMLButtonElement;
    this.errorBanner = document.getElementById("create-mount-error")!;

    this.bindEvents();
  }

  public setAvailableDrives(drives: VolumeInfoItem[]) {
    this.availableDrives = drives;
    this.renderQuickDrivePills();
  }

  private renderQuickDrivePills() {
    if (!this.quickDrivesContainer) return;
    this.quickDrivesContainer.innerHTML = "";

    this.availableDrives.forEach((d) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "drive-pill-btn";
      const label = d.filesystem_label ? `${d.mount_point} (${d.filesystem_label})` : d.mount_point;
      btn.textContent = label;
      btn.title = `Pre-populate base path from ${d.name || d.mount_point} (${d.filesystem})`;

      btn.addEventListener("click", () => {
        this.pathInput.value = d.mount_point;
        this.checkQuarantine(d.mount_point);
        // If name input is empty, propose a name from drive letter or label
        if (!this.nameInput.value.trim()) {
          const raw = (d.filesystem_label || d.name || d.mount_point)
            .toLowerCase()
            .replace(/[^a-z0-9_-]/g, "_")
            .replace(/_+/g, "_")
            .replace(/^_+|_+$/g, "");
          this.nameInput.value = raw || "storage_vault";
        }
      });

      this.quickDrivesContainer.appendChild(btn);
    });
  }

  private bindEvents() {
    this.closeBtn.addEventListener("click", () => this.hide());
    this.cancelBtn.addEventListener("click", () => this.hide());
    this.modalEl.querySelector(".modal-backdrop")?.addEventListener("click", () => this.hide());

    // Path input quarantine detector
    this.pathInput.addEventListener("input", (e) => {
      const val = (e.target as HTMLInputElement).value;
      this.checkQuarantine(val);
    });

    // Permission segmented buttons
    const permBtns = this.modalEl.querySelectorAll(".perm-toggle-btn");
    permBtns.forEach((btn) => {
      btn.addEventListener("click", () => {
        permBtns.forEach((b) => b.classList.remove("active"));
        btn.classList.add("active");
        this.selectedPermMode = (btn.getAttribute("data-perm") as any) || "read_only";
      });
    });

    // Subpath Tagging
    this.addTagBtn.addEventListener("click", () => this.handleAddTagFromInput());
    this.tagInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        this.handleAddTagFromInput();
      }
    });

    // Tag preset suggestions
    const presetPills = this.modalEl.querySelectorAll(".tag-preset-btn");
    presetPills.forEach((pill) => {
      pill.addEventListener("click", () => {
        const tag = pill.getAttribute("data-tag");
        if (tag) this.addTag(tag);
      });
    });

    // Folder preset suggestions (Videos, Movies, Music, Downloads)
    const folderPresets = this.modalEl.querySelectorAll(".folder-preset-btn");
    folderPresets.forEach((fp) => {
      fp.addEventListener("click", () => {
        const sub = fp.getAttribute("data-folder");
        if (!sub) return;
        const current = this.pathInput.value.trim().replace(/[\\/]+$/, "");
        if (current) {
          const sep = current.includes("/") ? "/" : "\\";
          this.pathInput.value = `${current}${sep}${sub}`;
        } else {
          this.pathInput.value = sub;
        }
        this.checkQuarantine(this.pathInput.value);
      });
    });

    // Form submit
    this.formEl.addEventListener("submit", async (e) => {
      e.preventDefault();
      await this.submit();
    });
  }

  private handleAddTagFromInput() {
    const val = this.tagInput.value.trim();
    if (val) {
      this.addTag(val);
      this.tagInput.value = "";
      this.tagInput.focus();
    }
  }

  public addTag(tag: string) {
    const clean = tag.replace(/^[/\\]+|[/\\]+$/g, "").trim();
    if (clean) {
      this.allowedSubpaths.add(clean);
      this.renderTags();
    }
  }

  public removeTag(tag: string) {
    this.allowedSubpaths.delete(tag);
    this.renderTags();
  }

  private renderTags() {
    this.tagsContainer.innerHTML = "";
    if (this.allowedSubpaths.size === 0) {
      this.tagsContainer.innerHTML = `<span class="empty-tags-hint">No whitelist tags specified (Entire root directory accessible)</span>`;
      return;
    }

    this.allowedSubpaths.forEach((tag) => {
      const chip = document.createElement("span");
      chip.className = "subpath-tag-chip";
      chip.innerHTML = `
        <span class="tag-label">&#128193; /${tag}</span>
        <button type="button" class="tag-remove-btn" title="Remove whitelist rule">&times;</button>
      `;
      chip.querySelector(".tag-remove-btn")?.addEventListener("click", (e) => {
        e.stopPropagation();
        this.removeTag(tag);
      });
      this.tagsContainer.appendChild(chip);
    });
  }

  private checkQuarantine(path: string): boolean {
    const p = path.trim().toLowerCase();
    const isQuarantined =
      p.startsWith("c:\\windows") ||
      p.startsWith("c:/windows") ||
      p.includes("system volume information") ||
      p === "/" ||
      p === "/etc" ||
      p.startsWith("/etc/") ||
      p === "/proc" ||
      p.startsWith("/proc/") ||
      p === "/sys" ||
      p.startsWith("/sys/") ||
      p === "/dev" ||
      p.startsWith("/dev/") ||
      p === "/boot" ||
      p.startsWith("/boot/");

    if (isQuarantined) {
      this.quarantineWarning.classList.remove("hidden");
    } else {
      this.quarantineWarning.classList.add("hidden");
    }
    return isQuarantined;
  }

  public show(prefill?: {
    physical_path?: string;
    name?: string;
    drive_serial?: string;
    description?: string;
  }) {
    this.reset();

    if (prefill) {
      if (prefill.physical_path) {
        this.pathInput.value = prefill.physical_path;
        this.checkQuarantine(prefill.physical_path);
      }
      if (prefill.name) {
        this.nameInput.value = prefill.name;
      } else if (prefill.physical_path) {
        const clean = prefill.physical_path
          .toLowerCase()
          .replace(/[^a-z0-9_-]/g, "_")
          .replace(/_+/g, "_")
          .replace(/^_+|_+$/g, "");
        this.nameInput.value = clean || "vault";
      }
      if (prefill.description) {
        this.descInput.value = prefill.description;
      }
    }

    this.modalEl.classList.remove("hidden");
    setTimeout(() => {
      this.nameInput.focus();
    }, 100);
  }

  public hide() {
    this.modalEl.classList.add("hidden");
    this.reset();
  }

  public reset() {
    this.formEl.reset();
    this.allowedSubpaths.clear();
    this.renderTags();
    this.errorBanner.classList.add("hidden");
    this.errorBanner.textContent = "";
    this.quarantineWarning.classList.add("hidden");
    this.selectedPermMode = "read_only";

    const permBtns = this.modalEl.querySelectorAll(".perm-toggle-btn");
    permBtns.forEach((b) => {
      b.classList.toggle("active", b.getAttribute("data-perm") === "read_only");
    });

    this.isSubmitting = false;
    this.submitBtn.disabled = false;
    this.submitBtn.innerHTML = `<span>+</span> Create Virtual Mount`;
  }

  private async submit() {
    if (this.isSubmitting) return;

    const name = this.nameInput.value.trim();
    const physicalPath = this.pathInput.value.trim();
    const description = this.descInput.value.trim() || undefined;
    const password = this.passwordInput.value.trim() || undefined;
    const expiryVal = parseInt(this.expirySelect.value, 10);
    const expirationTs = expiryVal > 0 ? Math.floor(Date.now() / 1000) + expiryVal : undefined;

    // Validation
    if (!name) {
      this.showError("Mount name / identifier is required.");
      this.nameInput.focus();
      return;
    }

    if (!/^[a-zA-Z0-9_-]+$/.test(name)) {
      this.showError("Mount name can only contain letters, numbers, hyphens, and underscores.");
      this.nameInput.focus();
      return;
    }

    if (!physicalPath) {
      this.showError("Physical host path is required.");
      this.pathInput.focus();
      return;
    }

    if (this.checkQuarantine(physicalPath)) {
      this.showError("Security Violation: This path is an OS-critical system path and is quarantined.");
      this.pathInput.focus();
      return;
    }

    this.isSubmitting = true;
    this.submitBtn.disabled = true;
    this.submitBtn.innerHTML = `<span class="spinning">&#8635;</span> Creating Mount...`;
    this.errorBanner.classList.add("hidden");

    const payload: CreateMountPayload = {
      name,
      physical_path: physicalPath,
      permission_mode: this.selectedPermMode,
      allowed_subpaths: this.allowedSubpaths.size > 0 ? Array.from(this.allowedSubpaths) : undefined,
      description,
      expiration_ts: expirationTs,
      password,
    };

    try {
      const res = await AetherApi.createMount(payload);
      console.log("[CreateMount] Successfully created virtual mount:", res);

      if (this.onMountCreated) {
        this.onMountCreated(res.mount || res);
      }

      this.hide();
    } catch (err: any) {
      console.error("[CreateMount] Failed to create mount:", err);
      this.showError(err.message || "Failed to create virtual mount on server.");
    } finally {
      this.isSubmitting = false;
      this.submitBtn.disabled = false;
      this.submitBtn.innerHTML = `<span>+</span> Create Virtual Mount`;
    }
  }

  private showError(msg: string) {
    this.errorBanner.textContent = msg;
    this.errorBanner.classList.remove("hidden");
    this.errorBanner.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }
}
