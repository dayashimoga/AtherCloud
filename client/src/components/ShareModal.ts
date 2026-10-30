import { AetherApi, CreateShareLinkOptions, PublicShareDetailsResponse } from "../services/api";
import { ShareService } from "../services/shareService";

export class ShareModalComponent {
  private modalEl: HTMLElement;
  private targetDisplayEl: HTMLElement;
  private errorBanner: HTMLElement;
  private expiryPreviewEl: HTMLElement;
  private expiryPresetBtns: NodeListOf<HTMLButtonElement>;
  private customExpiryContainer: HTMLElement;
  private customExpiryValue: HTMLInputElement;
  private customExpiryUnit: HTMLSelectElement;
  private applyCustomExpiryBtn: HTMLButtonElement;
  private permBtns: NodeListOf<HTMLButtonElement>;
  private passkeyToggle: HTMLInputElement;
  private passkeyContainer: HTMLElement;
  private passkeyInput: HTMLInputElement;
  private togglePasskeyVisBtn: HTMLButtonElement;
  private generatePasskeyBtn: HTMLButtonElement;
  private generateBtn: HTMLButtonElement;
  private closeBtn: HTMLButtonElement;
  private cancelBtn: HTMLButtonElement;

  // Results
  private resultContainer: HTMLElement;
  private resBadgeExpiry: HTMLElement;
  private resBadgePasskey: HTMLElement;
  private resBadgePerm: HTMLElement;
  private urlInput: HTMLInputElement;
  private copyBtn: HTMLButtonElement;
  private webShareBtn: HTMLButtonElement | null;
  private streamUrlInput: HTMLInputElement | null;
  private copyStreamBtn: HTMLButtonElement | null;
  private downloadUrlInput: HTMLInputElement | null;
  private copyDownloadBtn: HTMLButtonElement | null;
  private qrToggleBtn: HTMLButtonElement | null;
  private qrContainer: HTMLElement | null;
  private qrVisual: HTMLElement | null;

  private activeMount: string | null = null;
  private activePath: string | null = null;
  private selectedTtlSeconds = 86400; // 24 hours
  private selectedPermMode: "stream_only" | "read_only" | "read_write" = "read_only";
  private passkeyEnabled = false;
  private passkeyVisible = false;
  private isGenerating = false;

  constructor() {
    this.modalEl = document.getElementById("share-modal")!;
    this.targetDisplayEl = document.getElementById("share-target-display")!;
    this.errorBanner = document.getElementById("share-error-banner")!;
    this.expiryPreviewEl = document.getElementById("share-expiry-preview")!;
    this.expiryPresetBtns = this.modalEl.querySelectorAll(".expiry-preset-btn");
    this.customExpiryContainer = document.getElementById("custom-expiry-container")!;
    this.customExpiryValue = document.getElementById("custom-expiry-value") as HTMLInputElement;
    this.customExpiryUnit = document.getElementById("custom-expiry-unit") as HTMLSelectElement;
    this.applyCustomExpiryBtn = document.getElementById("apply-custom-expiry-btn") as HTMLButtonElement;
    this.permBtns = this.modalEl.querySelectorAll(".share-perm-btn");
    this.passkeyToggle = document.getElementById("share-passkey-toggle") as HTMLInputElement;
    this.passkeyContainer = document.getElementById("share-passkey-container")!;
    this.passkeyInput = document.getElementById("share-passkey-input") as HTMLInputElement;
    this.togglePasskeyVisBtn = document.getElementById("btn-toggle-passkey-vis") as HTMLButtonElement;
    this.generatePasskeyBtn = document.getElementById("btn-generate-passkey") as HTMLButtonElement;
    this.generateBtn = document.getElementById("generate-share-btn") as HTMLButtonElement;
    this.closeBtn = document.getElementById("close-share-btn") as HTMLButtonElement;
    this.cancelBtn = document.getElementById("cancel-share-btn") as HTMLButtonElement;

    this.resultContainer = document.getElementById("share-result")!;
    this.resBadgeExpiry = document.getElementById("res-badge-expiry")!;
    this.resBadgePasskey = document.getElementById("res-badge-passkey")!;
    this.resBadgePerm = document.getElementById("res-badge-perm")!;
    this.urlInput = document.getElementById("share-url-input") as HTMLInputElement;
    this.copyBtn = document.getElementById("copy-share-btn") as HTMLButtonElement;
    this.webShareBtn = document.getElementById("web-share-btn") as HTMLButtonElement | null;
    this.streamUrlInput = document.getElementById("share-stream-url") as HTMLInputElement | null;
    this.copyStreamBtn = document.getElementById("copy-stream-btn") as HTMLButtonElement | null;
    this.downloadUrlInput = document.getElementById("share-download-url") as HTMLInputElement | null;
    this.copyDownloadBtn = document.getElementById("copy-download-btn") as HTMLButtonElement | null;
    this.qrToggleBtn = document.getElementById("share-toggle-qr-btn") as HTMLButtonElement | null;
    this.qrContainer = document.getElementById("share-qr-container");
    this.qrVisual = document.getElementById("share-qr-visual");

    this.bindEvents();
  }

  private bindEvents() {
    this.closeBtn.addEventListener("click", () => this.hide());
    this.cancelBtn.addEventListener("click", () => this.hide());
    this.modalEl.querySelector(".modal-backdrop")?.addEventListener("click", () => this.hide());

    // Expiration presets
    this.expiryPresetBtns.forEach((btn) => {
      btn.addEventListener("click", () => {
        const ttlAttr = btn.getAttribute("data-ttl");
        this.expiryPresetBtns.forEach((b) => b.classList.remove("active"));
        btn.classList.add("active");

        if (ttlAttr === "custom") {
          this.customExpiryContainer.classList.remove("hidden");
          this.applyCustomExpiry();
        } else {
          this.customExpiryContainer.classList.add("hidden");
          this.selectedTtlSeconds = parseInt(ttlAttr || "86400", 10);
          this.updateExpiryPreview();
        }
      });
    });

    // Custom expiration apply button
    this.applyCustomExpiryBtn.addEventListener("click", () => this.applyCustomExpiry());
    this.customExpiryValue.addEventListener("input", () => this.applyCustomExpiry());
    this.customExpiryUnit.addEventListener("change", () => this.applyCustomExpiry());

    // Permission segmented buttons
    this.permBtns.forEach((btn) => {
      btn.addEventListener("click", () => {
        this.permBtns.forEach((b) => b.classList.remove("active"));
        btn.classList.add("active");
        this.selectedPermMode = (btn.getAttribute("data-perm") as any) || "read_only";
      });
    });

    // Passkey toggle
    this.passkeyToggle.addEventListener("change", () => {
      this.passkeyEnabled = this.passkeyToggle.checked;
      if (this.passkeyEnabled) {
        this.passkeyContainer.classList.remove("hidden");
        if (!this.passkeyInput.value.trim()) {
          this.generateRandomPasskey();
        }
        this.passkeyInput.focus();
      } else {
        this.passkeyContainer.classList.add("hidden");
      }
    });

    // Passkey random generator
    this.generatePasskeyBtn.addEventListener("click", () => {
      this.generateRandomPasskey();
    });

    // Passkey visibility toggle
    this.togglePasskeyVisBtn.addEventListener("click", () => {
      this.passkeyVisible = !this.passkeyVisible;
      this.passkeyInput.type = this.passkeyVisible ? "text" : "password";
      this.togglePasskeyVisBtn.innerHTML = this.passkeyVisible ? "&#128064;" : "&#128065;";
    });

    // Generate Share Link Button
    this.generateBtn.addEventListener("click", () => this.generateShareLink());

    // Copy handlers
    this.wireCopyButton(this.copyBtn, this.urlInput);
    if (this.copyStreamBtn && this.streamUrlInput) {
      this.wireCopyButton(this.copyStreamBtn, this.streamUrlInput);
    }
    if (this.copyDownloadBtn && this.downloadUrlInput) {
      this.wireCopyButton(this.copyDownloadBtn, this.downloadUrlInput);
    }

    // Web Share API native sheet integration with fallback
    if (this.webShareBtn) {
      this.webShareBtn.addEventListener("click", async () => {
        if (!this.urlInput.value) return;
        await ShareService.getInstance().share({
          title: `AetherStream: ${this.activePath || "Shared Media"}`,
          text: `Stream or download media from AetherStream`,
          url: this.urlInput.value,
        });
      });
    }

    // QR Code toggle
    if (this.qrToggleBtn && this.qrContainer) {
      this.qrToggleBtn.addEventListener("click", () => {
        const isHidden = this.qrContainer!.classList.contains("hidden");
        if (isHidden) {
          this.qrContainer!.classList.remove("hidden");
          this.qrToggleBtn!.innerHTML = `<span>&#128247;</span> Hide QR Code`;
          this.renderQrCode();
        } else {
          this.qrContainer!.classList.add("hidden");
          this.qrToggleBtn!.innerHTML = `<span>&#128247;</span> Show Connection QR Code`;
        }
      });
    }
  }

  private wireCopyButton(btn: HTMLButtonElement, input: HTMLInputElement) {
    btn.addEventListener("click", async () => {
      if (!input.value) return;
      const res = await ShareService.getInstance().copyToClipboard(input.value);
      if (res.success) {
        const orig = btn.innerHTML;
        btn.innerHTML = "&#10003; Copied!";
        btn.classList.add("copied-pulse");
        setTimeout(() => {
          btn.innerHTML = orig;
          btn.classList.remove("copied-pulse");
        }, 2000);
      }
    });
  }

  private applyCustomExpiry() {
    const num = Math.max(1, parseInt(this.customExpiryValue.value, 10) || 1);
    const unit = this.customExpiryUnit.value;
    const mult = unit === "days" ? 86400 : 3600;
    this.selectedTtlSeconds = num * mult;
    this.updateExpiryPreview();
  }

  private updateExpiryPreview() {
    const expireDate = new Date(Date.now() + this.selectedTtlSeconds * 1000);
    const hours = Math.round(this.selectedTtlSeconds / 3600);
    const days = (this.selectedTtlSeconds / 86400).toFixed(1);

    const durationStr =
      this.selectedTtlSeconds < 86400
        ? `${hours} hour${hours === 1 ? "" : "s"}`
        : `${days} days`;

    const dateFormatted = expireDate.toLocaleString(undefined, {
      weekday: "short",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });

    this.expiryPreviewEl.textContent = `Expires ${dateFormatted} (in ${durationStr})`;
  }

  private generateRandomPasskey() {
    const chars = "abcdefghjkmnpqrstuvwxyz23456789";
    let code = "";
    for (let i = 0; i < 8; i++) {
      code += chars.charAt(Math.floor(Math.random() * chars.length));
    }
    this.passkeyInput.value = code;
  }

  public show(mount: string, path: string) {
    this.activeMount = mount;
    this.activePath = path;
    const fileName = path.split("/").pop() || path;
    this.targetDisplayEl.textContent = `Target: ${mount} / ${fileName}`;

    this.reset();
    this.modalEl.classList.remove("hidden");
  }

  public hide() {
    this.modalEl.classList.add("hidden");
    this.reset();
  }

  private reset() {
    this.selectedTtlSeconds = 86400;
    this.selectedPermMode = "read_only";
    this.passkeyEnabled = false;
    this.passkeyVisible = false;
    this.passkeyToggle.checked = false;
    this.passkeyInput.value = "";
    this.passkeyInput.type = "password";
    this.passkeyContainer.classList.add("hidden");
    this.customExpiryContainer.classList.add("hidden");
    this.errorBanner.classList.add("hidden");
    this.errorBanner.textContent = "";
    this.resultContainer.classList.add("hidden");
    this.urlInput.value = "";
    if (this.streamUrlInput) this.streamUrlInput.value = "";
    if (this.downloadUrlInput) this.downloadUrlInput.value = "";
    if (this.qrContainer) this.qrContainer.classList.add("hidden");

    this.expiryPresetBtns.forEach((b) => {
      b.classList.toggle("active", b.getAttribute("data-ttl") === "86400");
    });

    this.permBtns.forEach((b) => {
      b.classList.toggle("active", b.getAttribute("data-perm") === "read_only");
    });

    this.updateExpiryPreview();
    this.isGenerating = false;
    this.generateBtn.disabled = false;
    this.generateBtn.innerHTML = `<span class="btn-icon">&#128279;</span> Generate Signed Share Link`;
  }

  private async generateShareLink() {
    if (this.isGenerating || !this.activeMount || !this.activePath) return;

    if (this.passkeyEnabled && !this.passkeyInput.value.trim()) {
      this.showError("Passkey protection is enabled. Please enter or generate a passkey.");
      this.passkeyInput.focus();
      return;
    }

    this.isGenerating = true;
    this.generateBtn.disabled = true;
    this.generateBtn.innerHTML = `<span class="spinning">&#8635;</span> Signing HMAC Token...`;
    this.errorBanner.classList.add("hidden");

    const passkeyVal = this.passkeyEnabled ? this.passkeyInput.value.trim() : undefined;

    const opts: CreateShareLinkOptions = {
      mount: this.activeMount,
      path: this.activePath,
      ttl_seconds: this.selectedTtlSeconds,
      permission_mode: this.selectedPermMode,
      passkey: passkeyVal,
      description: `Public share for ${this.activePath}`,
    };

    try {
      const details: PublicShareDetailsResponse = await AetherApi.createShareLink(opts);
      console.log("[PublicShare] Generated share link:", details);

      const origin = window.location.origin;
      const fullShareUrl = details.share_url.startsWith("http")
        ? details.share_url
        : `${origin}${details.share_url}`;
      const fullStreamUrl = details.stream_url.startsWith("http")
        ? details.stream_url
        : `${origin}${details.stream_url}`;
      const fullDownloadUrl = details.download_url.startsWith("http")
        ? details.download_url
        : `${origin}${details.download_url}`;

      this.urlInput.value = fullShareUrl;
      if (this.streamUrlInput) this.streamUrlInput.value = fullStreamUrl;
      if (this.downloadUrlInput) this.downloadUrlInput.value = fullDownloadUrl;

      // Badges
      const hours = Math.round(details.ttl_seconds / 3600);
      this.resBadgeExpiry.textContent = `${hours}h Expiration`;
      this.resBadgePasskey.textContent = details.has_passkey ? "🔒 Passkey Protected" : "🌐 Open Access";
      this.resBadgePasskey.className = `badge ${details.has_passkey ? "status-warning" : "status-healthy"}`;
      this.resBadgePerm.textContent =
        details.permission_mode === "stream_only"
          ? "Stream-Only"
          : details.permission_mode === "read_write"
          ? "Read & Write"
          : "Read-Only";

      this.resultContainer.classList.remove("hidden");
      this.resultContainer.scrollIntoView({ behavior: "smooth", block: "nearest" });
    } catch (err: any) {
      console.error("[PublicShare] Failed to create share link:", err);
      this.showError(err.message || "Failed to generate signed share token on server.");
    } finally {
      this.isGenerating = false;
      this.generateBtn.disabled = false;
      this.generateBtn.innerHTML = `<span class="btn-icon">&#128279;</span> Generate Signed Share Link`;
    }
  }

  private renderQrCode() {
    if (!this.qrVisual || !this.urlInput.value) return;
    const targetUrl = this.urlInput.value;

    const qrApiUrl = `/api/v1/tunnel/qr?target=wan&format=svg`;
    this.qrVisual.innerHTML = `
      <div class="qr-loading">Loading vector QR code...</div>
      <img src="${qrApiUrl}" alt="Share QR Code" class="qr-img" onerror="this.onerror=null; this.parentElement.innerHTML='<div class=\\'qr-ascii-fallback\\'>📱 Scan link directly on mobile: <br/><code>${targetUrl}</code></div>';" />
    `;
  }

  private showError(msg: string) {
    this.errorBanner.textContent = msg;
    this.errorBanner.classList.remove("hidden");
  }
}
