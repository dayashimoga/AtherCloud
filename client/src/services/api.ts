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
  private static baseUrl = "";

  static async getHealth() {
    const res = await fetch(`${this.baseUrl}/api/v1/health`);
    if (!res.ok) throw new Error("Health check failed");
    return res.json();
  }

  static async getMounts(): Promise<MountItem[]> {
    const res = await fetch(`${this.baseUrl}/api/v1/mounts`);
    if (!res.ok) throw new Error("Failed to load mounts");
    const data = await res.json();
    return data.mounts;
  }

  static async getSystemDrives(): Promise<DiscoveredDriveItem[]> {
    const res = await fetch(`${this.baseUrl}/api/v1/system/drives`);
    if (!res.ok) throw new Error("Failed to load system drives");
    const data = await res.json();
    return data.drives;
  }

  static async browse(mount: string, path = ""): Promise<BrowseResponse> {
    const sanitizedPath = path.replace(/^\/+/, "");
    const url = sanitizedPath
      ? `${this.baseUrl}/api/v1/browse/${mount}/${sanitizedPath}`
      : `${this.baseUrl}/api/v1/browse/${mount}`;
    const res = await fetch(url);
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
      headers: { "Content-Type": "application/json" },
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
    const base = `${this.baseUrl}/api/v1/stream/${mount}/${path.replace(/^\/+/, "")}`;
    return token ? `${base}?token=${encodeURIComponent(token)}` : base;
  }

  static getHlsMasterUrl(mount: string, path: string, token?: string): string {
    const base = `${this.baseUrl}/api/v1/transcode/hls/${mount}/${path.replace(/^\/+/, "")}/master.m3u8`;
    return token ? `${base}?token=${encodeURIComponent(token)}` : base;
  }
}
