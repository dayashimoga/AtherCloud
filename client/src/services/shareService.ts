export type ShareMethod =
  | "web_share"
  | "clipboard"
  | "clipboard_fallback"
  | "dismissed"
  | "failed";

export interface SharePayload {
  title?: string;
  text?: string;
  url: string;
}

export interface ShareResult {
  success: boolean;
  method: ShareMethod;
  error?: string;
}

export class ShareService {
  private static instance: ShareService | null = null;
  private toastTimeoutId: number | null = null;

  public static getInstance(): ShareService {
    if (!ShareService.instance) {
      ShareService.instance = new ShareService();
    }
    return ShareService.instance;
  }

  public isWebShareSupported(): boolean {
    return (
      typeof navigator !== "undefined" &&
      typeof navigator.share === "function" &&
      (typeof window === "undefined" || window.isSecureContext !== false)
    );
  }

  public canShare(payload?: SharePayload): boolean {
    if (!this.isWebShareSupported()) return false;
    if (!payload) return true;
    if (typeof navigator.canShare === "function") {
      try {
        return navigator.canShare(payload);
      } catch {
        return true;
      }
    }
    return true;
  }

  public isClipboardSupported(): boolean {
    return (
      typeof navigator !== "undefined" &&
      Boolean(navigator.clipboard && typeof navigator.clipboard.writeText === "function")
    );
  }

  public async share(payload: SharePayload): Promise<ShareResult> {
    if (this.canShare(payload)) {
      try {
        await navigator.share({
          title: payload.title || "AetherStream Media",
          text: payload.text || "Shared from AetherStream",
          url: payload.url,
        });
        this.triggerHaptic([20, 30, 20]);
        this.showToast("Shared successfully!");
        return { success: true, method: "web_share" };
      } catch (err: any) {
        if (err.name === "AbortError") {
          return { success: false, method: "dismissed" };
        }
        console.warn("[ShareService] navigator.share failed, falling back to clipboard:", err);
      }
    }

    return this.copyToClipboard(payload.url);
  }

  public async copyToClipboard(text: string): Promise<ShareResult> {
    if (this.isClipboardSupported()) {
      try {
        await navigator.clipboard.writeText(text);
        this.triggerHaptic([20, 30, 20]);
        this.showToast("Link copied to clipboard!");
        return { success: true, method: "clipboard" };
      } catch (err: any) {
        console.warn("[ShareService] navigator.clipboard.writeText failed, trying execCommand fallback:", err);
      }
    }

    return this.fallbackExecCommand(text);
  }

  public fallbackExecCommand(text: string): ShareResult {
    try {
      const textarea = document.createElement("textarea");
      textarea.value = text;
      textarea.setAttribute("readonly", "");
      textarea.style.position = "fixed";
      textarea.style.top = "-9999px";
      textarea.style.left = "-9999px";
      textarea.style.opacity = "0";
      document.body.appendChild(textarea);
      textarea.focus();
      textarea.select();

      const successful = document.execCommand("copy");
      document.body.removeChild(textarea);

      if (successful) {
        this.triggerHaptic([20, 30, 20]);
        this.showToast("Link copied to clipboard!");
        return { success: true, method: "clipboard_fallback" };
      }
    } catch (err: any) {
      console.error("[ShareService] Ephemeral execCommand copy failed:", err);
    }

    this.showToast("Unable to copy link to clipboard.");
    return { success: false, method: "failed", error: "Clipboard write failed" };
  }

  public triggerHaptic(pattern: number[] = [20, 30, 20]): void {
    if (
      typeof navigator !== "undefined" &&
      "vibrate" in navigator &&
      typeof navigator.vibrate === "function"
    ) {
      try {
        navigator.vibrate(pattern);
      } catch {
        // Ignored on platforms without vibration hardware
      }
    }
  }

  public showToast(message: string, durationMs = 2500): void {
    let toastEl = document.getElementById("global-toast");
    if (!toastEl) {
      toastEl = document.createElement("div");
      toastEl.id = "global-toast";
      toastEl.className = "global-toast hidden";
      document.body.appendChild(toastEl);
    }

    toastEl.textContent = message;
    toastEl.classList.remove("hidden");
    toastEl.classList.add("show");

    if (this.toastTimeoutId !== null) {
      window.clearTimeout(this.toastTimeoutId);
    }

    this.toastTimeoutId = window.setTimeout(() => {
      toastEl?.classList.remove("show");
      toastEl?.classList.add("hidden");
      this.toastTimeoutId = null;
    }, durationMs);
  }
}

