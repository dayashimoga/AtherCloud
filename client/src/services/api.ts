import { SessionManager } from "./session";

export interface MountItem {
  id: string;
  read_only: boolean;
  description: string | null;
  path_display: string;
}

export interface DiscoveredDriveItem {
  path: string;
  name: string;
  drive_type: string;
  is_system: boolean;
  is_removable: boolean;
}

export interface FileItem {
  name: string;
  is_dir: boolean;
  size: number;
  modified_utc: string | null;
  mime_type: string;
}

export interface BrowseResponse {
  mount: string;
  path: string;
  items: FileItem[];
}

export class AetherApi {
  private static get baseUrl(): string {
    return SessionManager.getInstance().getServerUrl();
  }

  private static getHeaders(custom: Record<string, string> = {}): Record<string, string> {
    return {
      ...SessionManager.getInstance().getAuthHeaders(),
      ...custom,
    };
  }

  static async getHealth() {
    const res = await fetch(`${this.baseUrl}/api/v1/health`, {
      headers: this.getHeaders(),
    });
    if (!res.ok) throw new Error("Health check failed");
    return res.json();
  }

  static async getMounts(): Promise<MountItem[]> {
    const res = await fetch(`${this.baseUrl}/api/v1/mounts`, {
      headers: this.getHeaders(),
    });
    if (!res.ok) throw new Error("Failed to load mounts");
    const data = await res.json();
    return data.mounts;
  }

  static async getSystemDrives(): Promise<DiscoveredDriveItem[]> {
    const res = await fetch(`${this.baseUrl}/api/v1/system/drives`, {
      headers: this.getHeaders(),
    });
    if (!res.ok) throw new Error("Failed to load system drives");
    const data = await res.json();
    return data.drives;
  }

  static async browse(mount: string, path = ""): Promise<BrowseResponse> {
    const sanitizedPath = path.replace(/^\/+/, "");
    const url = sanitizedPath
      ? `${this.baseUrl}/api/v1/browse/${mount}/${sanitizedPath}`
      : `${this.baseUrl}/api/v1/browse/${mount}`;
    const res = await fetch(url, {
      headers: this.getHeaders(),
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({ error: "Browse failed" }));
      throw new Error(err.error || "Failed to browse directory");
    }
    return res.json();
  }

  static async createShareLink(
    mount: string,
    path: string,
    ttlSeconds = 86400,
    allowWrite = false
  ) {
    const res = await fetch(`${this.baseUrl}/api/v1/share/create`, {
      method: "POST",
      headers: this.getHeaders({ "Content-Type": "application/json" }),
      body: JSON.stringify({
        mount,
        path,
        ttl_seconds: ttlSeconds,
        allow_write: allowWrite,
      }),
    });
    if (!res.ok) throw new Error("Failed to create share link");
    return res.json();
  }

  static getStreamUrl(mount: string, path: string, token?: string): string {
    const effectiveToken = token || SessionManager.getInstance().getToken();
    const base = `${this.baseUrl}/api/v1/stream/${mount}/${path.replace(/^\/+/, "")}`;
    return effectiveToken ? `${base}?token=${encodeURIComponent(effectiveToken)}` : base;
  }

  static getDownloadUrl(mount: string, path: string, token?: string): string {
    const effectiveToken = token || SessionManager.getInstance().getToken();
    const base = `${this.baseUrl}/api/v1/stream/${mount}/${path.replace(/^\/+/, "")}?download=true`;
    return effectiveToken ? `${base}&token=${encodeURIComponent(effectiveToken)}` : base;
  }

  static getHlsMasterUrl(mount: string, path: string, token?: string): string {
    const effectiveToken = token || SessionManager.getInstance().getToken();
    const base = `${this.baseUrl}/api/v1/transcode/hls/${mount}/${path.replace(/^\/+/, "")}/master.m3u8`;
    return effectiveToken ? `${base}?token=${encodeURIComponent(effectiveToken)}` : base;
  }

  static async computeSha256(buffer: ArrayBuffer): Promise<string> {
    const hashBuffer = await crypto.subtle.digest("SHA-256", buffer);
    const hashArray = Array.from(new Uint8Array(hashBuffer));
    return hashArray.map((b) => b.toString(16).padStart(2, "0")).join("");
  }

  static async initUpload(
    mount: string,
    path: string,
    totalSize: number,
    chunkSize = 2 * 1024 * 1024
  ): Promise<{ upload_id: string; total_chunks: number; chunk_size: number; total_size: number }> {
    const uploadId = "upl_" + Math.random().toString(36).substring(2, 10) + "_" + Date.now();
    const res = await fetch(`${this.baseUrl}/api/v1/upload/${mount}/init`, {
      method: "POST",
      headers: this.getHeaders({ "Content-Type": "application/json" }),
      body: JSON.stringify({
        upload_id: uploadId,
        path,
        total_size: totalSize,
        chunk_size: chunkSize,
      }),
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({ error: "Upload init failed" }));
      throw new Error(err.error || "Failed to initialize upload session");
    }
    return res.json();
  }

  static async uploadChunk(
    mount: string,
    uploadId: string,
    chunkIndex: number,
    sha256: string,
    chunkBytes: ArrayBuffer | Blob
  ) {
    const url = `${this.baseUrl}/api/v1/upload/${mount}/chunk?upload_id=${encodeURIComponent(
      uploadId
    )}&chunk_index=${chunkIndex}&sha256=${encodeURIComponent(sha256)}`;

    const res = await fetch(url, {
      method: "POST",
      headers: this.getHeaders({ "Content-Type": "application/octet-stream" }),
      body: chunkBytes,
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({ error: "Chunk upload failed" }));
      throw new Error(err.error || `Failed to upload chunk ${chunkIndex}`);
    }
    return res.json();
  }

  static async completeUpload(mount: string, uploadId: string, expectedSha256?: string) {
    const res = await fetch(`${this.baseUrl}/api/v1/upload/${mount}/complete`, {
      method: "POST",
      headers: this.getHeaders({ "Content-Type": "application/json" }),
      body: JSON.stringify({
        upload_id: uploadId,
        expected_sha256: expectedSha256 || null,
      }),
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({ error: "Upload completion failed" }));
      throw new Error(err.error || "Failed to complete upload");
    }
    return res.json();
  }
}
