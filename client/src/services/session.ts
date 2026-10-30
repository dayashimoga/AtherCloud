export type ThemeMode = "auto" | "dark" | "light";

export interface SessionConfig {
  token: string | null;
  serverUrl: string;
  theme: ThemeMode;
  autoPlayNext: boolean;
  volume: number;
}

const STORAGE_KEY = "aetherstream_session_v1";

const DEFAULT_CONFIG: SessionConfig = {
  token: null,
  serverUrl: "",
  theme: "dark",
  autoPlayNext: true,
  volume: 1.0,
};

export class SessionManager {
  private static instance: SessionManager;
  private state: SessionConfig;
  private memoryFallback = false;

  private constructor() {
    this.state = this.loadState();
    this.applyTheme(this.state.theme);
  }

  public static getInstance(): SessionManager {
    if (!SessionManager.instance) {
      SessionManager.instance = new SessionManager();
    }
    return SessionManager.instance;
  }

  private loadState(): SessionConfig {
    try {
      const raw = localStorage.getItem(STORAGE_KEY);
      if (raw) {
        return { ...DEFAULT_CONFIG, ...JSON.parse(raw) };
      }
    } catch {
      this.memoryFallback = true;
    }
    return { ...DEFAULT_CONFIG };
  }

  public saveState(partial: Partial<SessionConfig>) {
    this.state = { ...this.state, ...partial };
    if (!this.memoryFallback) {
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(this.state));
      } catch {
        this.memoryFallback = true;
      }
    }
    if (partial.theme !== undefined) {
      this.applyTheme(partial.theme);
    }
  }

  public getState(): Readonly<SessionConfig> {
    return this.state;
  }

  public getToken(): string | null {
    return this.state.token;
  }

  public setToken(token: string | null) {
    this.saveState({ token });
  }

  public getServerUrl(): string {
    return this.state.serverUrl;
  }

  public setServerUrl(url: string) {
    this.saveState({ serverUrl: url.replace(/\/+$/, "") });
  }

  public getTheme(): ThemeMode {
    return this.state.theme;
  }

  public toggleTheme(): ThemeMode {
    const nextTheme: ThemeMode = this.state.theme === "dark" ? "light" : "dark";
    this.saveState({ theme: nextTheme });
    return nextTheme;
  }

  public applyTheme(theme: ThemeMode) {
    const root = document.documentElement;
    if (theme === "auto") {
      const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
      root.setAttribute("data-theme", prefersDark ? "dark" : "light");
    } else {
      root.setAttribute("data-theme", theme);
    }
  }

  public getAuthHeaders(): Record<string, string> {
    const headers: Record<string, string> = {};
    if (this.state.token) {
      headers["Authorization"] = `Bearer ${this.state.token}`;
      headers["X-Aether-Token"] = this.state.token;
    }
    return headers;
  }

  public clearSession() {
    this.state = { ...DEFAULT_CONFIG };
    try {
      localStorage.removeItem(STORAGE_KEY);
    } catch {
      // Ignored
    }
    this.applyTheme(DEFAULT_CONFIG.theme);
  }
}
