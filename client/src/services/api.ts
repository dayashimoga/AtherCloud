import { SessionManager } from "./session";

export interface MountItem {
  id: string;
  read_only: boolean;
  description: string | null;
  path_display: string;
}

export type DiskMediaType =
  | "NvmeSsd"
  | "SataSsd"
  | "RotationalHdd"
  | "RemovableUsb"
  | "OpticalDrive"
  | "NetworkDrive"
  | "Unknown";

export interface VolumeInfoItem {
  name: string;
  mount_point: string;
  filesystem: string;
  filesystem_label: string | null;
  total_bytes: number;
  available_bytes: number;
  is_removable: boolean;
  is_system: boolean;
  media_type: DiskMediaType | string;
  serial_number: string | null;
  is_read_only: boolean;
  // Backward compatibility alias:
  path?: string;
  drive_type?: string;
}

export type DiscoveredDriveItem = VolumeInfoItem;

export interface CreateMountPayload {
  id?: string;
  name: string;
  physical_path: string;
  permission_mode?: "read_only" | "read_write" | "stream_only";
  allowed_subpaths?: string[];
  drive_serial?: string;
  description?: string;
  expiration_ts?: number;
  password?: string;
}

export interface CreateShareLinkOptions {
  mount: string;
  path: string;
  ttl_seconds?: number;
  permission_mode?: "stream_only" | "read_only" | "read_write";
  passkey?: string;
  description?: string;
  allow_write?: boolean;
}

export interface PublicShareDetailsResponse {
  share_id: string;
  token: string;
  mount: string;
  path: string;
  permission_mode: string;
  issued_at: number;
  expires_at: number;
  ttl_seconds: number;
  has_passkey: boolean;
  share_url: string;
  stream_url: string;
  download_url: string;
  browse_url: string;
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

export interface TranscodeStatusInfo {
  active_accel: string;
  active_encoder: string;
  active_hevc_encoder?: string | null;
  active_av1_encoder?: string | null;
  is_hardware_accelerated: boolean;
  description: string;
  extra_ffmpeg_flags?: string[];
  os_platform: string;
  dri_device_accessible?: boolean;
  status: string;
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

  static async getTranscodeStatus(): Promise<TranscodeStatusInfo | null> {
    try {
      const res = await fetch(`${this.baseUrl}/api/v1/transcode/status`, {
        headers: this.getHeaders(),
      });
      if (res.ok) {
        return await res.json();
      }
    } catch (err) {
      console.warn("Failed to fetch transcode status:", err);
    }
    return null;
  }

  static async getMounts(): Promise<MountItem[]> {
    const res = await fetch(`${this.baseUrl}/api/v1/mounts`, {
      headers: this.getHeaders(),
    });
    if (!res.ok) throw new Error("Failed to load mounts");
    const data = await res.json();
    return data.mounts;
  }

  static async createMount(payload: CreateMountPayload): Promise<any> {
    const id = payload.id || `mnt_${payload.name.toLowerCase().replace(/[^a-z0-9_]/g, "_")}_${Date.now().toString(36)}`;
    const res = await fetch(`${this.baseUrl}/api/v1/storage/mounts`, {
      method: "POST",
      headers: this.getHeaders({ "Content-Type": "application/json" }),
      body: JSON.stringify({
        ...payload,
        id,
      }),
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({ title: "Failed to create mount", detail: res.statusText }));
      throw new Error(err.detail || err.title || "Failed to create mount");
    }
    return res.json();
  }

  static async getStorageDrives(): Promise<VolumeInfoItem[]> {
    try {
      const res = await fetch(`${this.baseUrl}/api/v1/storage/drives`, {
        headers: this.getHeaders(),
      });
      if (res.ok) {
        const data = await res.json();
        const drives = Array.isArray(data.drives) ? data.drives : [];
        return drives.map((d: any) => this.normalizeVolumeInfo(d));
      }
    } catch {
      // Fallback to /api/v1/system/drives
    }
    return this.getSystemDrives();
  }

  static async getSystemDrives(): Promise<VolumeInfoItem[]> {
    const res = await fetch(`${this.baseUrl}/api/v1/system/drives`, {
      headers: this.getHeaders(),
    });
    if (!res.ok) throw new Error("Failed to load system drives");
    const data = await res.json();
    const drives = Array.isArray(data.drives) ? data.drives : [];
    return drives.map((d: any) => this.normalizeVolumeInfo(d));
  }

  private static normalizeVolumeInfo(d: any): VolumeInfoItem {
    const mountPoint = d.mount_point || d.path || "";
    const totalBytes = typeof d.total_bytes === "number" ? d.total_bytes : 0;
    const availBytes = typeof d.available_bytes === "number" ? d.available_bytes : 0;
    const isRemovable = Boolean(d.is_removable);
    const isSystem = Boolean(d.is_system);
    const mediaType = d.media_type || d.drive_type || "Unknown";
    return {
      name: d.name || mountPoint || "Storage Volume",
      mount_point: mountPoint,
      path: mountPoint,
      filesystem: d.filesystem || "Unknown",
      filesystem_label: d.filesystem_label ?? null,
      total_bytes: totalBytes,
      available_bytes: availBytes,
      is_removable: isRemovable,
      is_system: isSystem,
      media_type: mediaType,
      drive_type: mediaType,
      serial_number: d.serial_number ?? null,
      is_read_only: Boolean(d.is_read_only),
    };
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
    mountOrOpts: string | CreateShareLinkOptions,
    path?: string,
    ttlSeconds = 86400,
    allowWrite = false,
    passkey?: string,
    permMode?: "stream_only" | "read_only" | "read_write"
  ): Promise<PublicShareDetailsResponse> {
    let payload: Record<string, any>;

    if (typeof mountOrOpts === "object") {
      payload = {
        mount: mountOrOpts.mount,
        path: mountOrOpts.path,
        ttl_seconds: mountOrOpts.ttl_seconds ?? 86400,
        permission_mode:
          mountOrOpts.permission_mode ||
          (mountOrOpts.allow_write ? "read_write" : "read_only"),
        passkey: mountOrOpts.passkey || undefined,
        description: mountOrOpts.description,
      };
    } else {
      payload = {
        mount: mountOrOpts,
        path: path || "",
        ttl_seconds: ttlSeconds,
        permission_mode: permMode || (allowWrite ? "read_write" : "read_only"),
        passkey: passkey || undefined,
      };
    }

    const res = await fetch(`${this.baseUrl}/api/v1/share/create`, {
      method: "POST",
      headers: this.getHeaders({ "Content-Type": "application/json" }),
      body: JSON.stringify(payload),
    });
    if (!res.ok) {
      const err = await res
        .json()
        .catch(() => ({ title: "Failed to create share link", detail: res.statusText }));
      throw new Error(err.detail || err.title || "Failed to create share link");
    }
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
