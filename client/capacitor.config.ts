export interface CapacitorConfig {
  appId: string;
  appName: string;
  webDir: string;
  server?: {
    androidScheme?: string;
    cleartext?: boolean;
    url?: string;
  };
  android?: {
    buildOptions?: {
      releaseType?: string;
    };
  };
  ios?: {
    contentInset?: string;
    preferredContentMode?: string;
  };
  plugins?: Record<string, any>;
}

const config: CapacitorConfig = {
  appId: "io.aetherstream.app",
  appName: "AetherStream",
  webDir: "dist",
  server: {
    androidScheme: "https",
    cleartext: true,
  },
  android: {
    buildOptions: {
      releaseType: "AAB",
    },
  },
  ios: {
    contentInset: "automatic",
    preferredContentMode: "mobile",
  },
  plugins: {
    SplashScreen: {
      launchShowDuration: 1500,
      backgroundColor: "#0a0c10",
      showSpinner: false,
    },
    StatusBar: {
      backgroundColor: "#0a0c10",
      style: "DARK",
    },
  },
};

export default config;
