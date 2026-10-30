import { AetherApi } from "../services/api";

export class ShareModalComponent {
  private modalEl: HTMLElement;
  private targetDisplayEl: HTMLElement;
  private expirySelect: HTMLSelectElement;
  private permSelect: HTMLSelectElement;
  private generateBtn: HTMLButtonElement;
  private closeBtn: HTMLButtonElement;
  private resultContainer: HTMLElement;
  private urlInput: HTMLInputElement;
  private copyBtn: HTMLButtonElement;
  private downloadUrlInput: HTMLInputElement | null = null;
  private copyDownloadBtn: HTMLButtonElement | null = null;

  private activeMount: string | null = null;
  private activePath: string | null = null;

  constructor() {
    this.modalEl = document.getElementById("share-modal")!;
    this.targetDisplayEl = document.getElementById("share-target-display")!;
    this.expirySelect = document.getElementById("share-expiry") as HTMLSelectElement;
    this.permSelect = document.getElementById("share-perm") as HTMLSelectElement;
    this.generateBtn = document.getElementById("generate-share-btn") as HTMLButtonElement;
    this.closeBtn = document.getElementById("close-share-btn") as HTMLButtonElement;
    this.resultContainer = document.getElementById("share-result")!;
    this.urlInput = document.getElementById("share-url-input") as HTMLInputElement;
    this.copyBtn = document.getElementById("copy-share-btn") as HTMLButtonElement;
    this.downloadUrlInput = document.getElementById("share-download-url") as HTMLInputElement | null;
    this.copyDownloadBtn = document.getElementById("copy-download-btn") as HTMLButtonElement | null;

    this.bindEvents();
  }

  private bindEvents() {
    this.closeBtn.onclick = () => this.hide();
    this.modalEl.querySelector(".modal-backdrop")!.addEventListener("click", () => this.hide());

    this.generateBtn.onclick = async () => {
      if (!this.activeMount || !this.activePath) return;

      const ttl = parseInt(this.expirySelect.value, 10);
      const allowWrite = this.permSelect.value === "true";

      this.generateBtn.disabled = true;
      this.generateBtn.textContent = "Signing HMAC Token...";

      try {
        const res = await AetherApi.createShareLink(
          this.activeMount,
          this.activePath,
          ttl,
          allowWrite
        );
        const fullStreamUrl = `${window.location.origin}${res.stream_url}`;
        this.urlInput.value = fullStreamUrl;

        if (this.downloadUrlInput && res.download_url) {
          this.downloadUrlInput.value = `${window.location.origin}${res.download_url}`;
        }

        this.resultContainer.classList.remove("hidden");
      } catch (err: any) {
        alert(`Failed to create share link: ${err.message}`);
      } finally {
        this.generateBtn.disabled = false;
        this.generateBtn.textContent = "Generate HMAC Token";
      }
    };

    this.copyBtn.onclick = () => {
      navigator.clipboard.writeText(this.urlInput.value).then(() => {
        this.copyBtn.textContent = "Copied!";
        setTimeout(() => {
          this.copyBtn.textContent = "Copy";
        }, 2000);
      });
    };

    if (this.copyDownloadBtn && this.downloadUrlInput) {
      this.copyDownloadBtn.onclick = () => {
        navigator.clipboard.writeText(this.downloadUrlInput!.value).then(() => {
          this.copyDownloadBtn!.textContent = "Copied!";
          setTimeout(() => {
            this.copyDownloadBtn!.textContent = "Copy";
          }, 2000);
        });
      };
    }
  }

  show(mount: string, path: string) {
    this.activeMount = mount;
    this.activePath = path;
    this.targetDisplayEl.textContent = `Target: ${mount}/${path}`;
    this.resultContainer.classList.add("hidden");
    this.urlInput.value = "";
    if (this.downloadUrlInput) {
      this.downloadUrlInput.value = "";
    }
    this.modalEl.classList.remove("hidden");
  }

  hide() {
    this.modalEl.classList.add("hidden");
    this.activeMount = null;
    this.activePath = null;
  }
}
