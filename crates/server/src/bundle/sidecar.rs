use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SidecarBinary {
    FFmpeg,
    FFprobe,
    Cloudflared,
}

impl SidecarBinary {
    pub fn binary_name(self, is_windows: bool) -> &'static str {
        match (self, is_windows) {
            (SidecarBinary::FFmpeg, true) => "ffmpeg.exe",
            (SidecarBinary::FFmpeg, false) => "ffmpeg",
            (SidecarBinary::FFprobe, true) => "ffprobe.exe",
            (SidecarBinary::FFprobe, false) => "ffprobe",
            (SidecarBinary::Cloudflared, true) => "cloudflared.exe",
            (SidecarBinary::Cloudflared, false) => "cloudflared",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            SidecarBinary::FFmpeg => "ffmpeg",
            SidecarBinary::FFprobe => "ffprobe",
            SidecarBinary::Cloudflared => "cloudflared",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarBinaryInfo {
    pub name: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarManifest {
    pub schema_version: String,
    pub target_triple: String,
    pub bundled_at: String,
    pub binaries: BTreeMap<String, SidecarBinaryInfo>,
}

impl SidecarManifest {
    pub fn new(target_triple: &str) -> Self {
        Self {
            schema_version: "1.0.0".to_string(),
            target_triple: target_triple.to_string(),
            bundled_at: chrono_timestamp(),
            binaries: BTreeMap::new(),
        }
    }

    pub fn add_binary(&mut self, info: SidecarBinaryInfo) {
        self.binaries.insert(info.name.clone(), info);
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), std::io::Error> {
        let json = self.to_json().map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut file = File::create(path)?;
        file.write_all(json.as_bytes())?;
        Ok(())
    }
}

pub fn compute_sha256(path: &Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hex::encode(hasher.finalize()))
}

pub struct SidecarResolver;

impl SidecarResolver {
    /// Resolves the absolute path to a requested sidecar binary, searching:
    /// 1. Environment variable override (e.g. `AETHERSTREAM_SIDECARS_DIR` or `<NAME>_PATH`)
    /// 2. `./sidecars/<target>/` or `./sidecars/`
    /// 3. Directory adjacent to the current executable (`<exe_dir>/sidecars/`)
    /// 4. System `$PATH`
    pub fn resolve(binary: SidecarBinary, target_triple: Option<&str>) -> Option<PathBuf> {
        let is_windows = cfg!(windows) || target_triple.is_some_and(|t| t.contains("windows"));
        let bin_name = binary.binary_name(is_windows);

        // 1. Specific Env Var
        let env_key = match binary {
            SidecarBinary::FFmpeg => "FFMPEG_PATH",
            SidecarBinary::FFprobe => "FFPROBE_PATH",
            SidecarBinary::Cloudflared => "CLOUDFLARED_PATH",
        };
        if let Ok(val) = std::env::var(env_key) {
            let p = PathBuf::from(val);
            if p.is_file() {
                return Some(p);
            }
        }

        // 2. Global Sidecars Env Var
        if let Ok(dir) = std::env::var("AETHERSTREAM_SIDECARS_DIR") {
            let p = PathBuf::from(&dir).join(bin_name);
            if p.is_file() {
                return Some(p);
            }
            if let Some(target) = target_triple {
                let target_p = PathBuf::from(&dir).join(target).join(bin_name);
                if target_p.is_file() {
                    return Some(target_p);
                }
            }
        }

        // 3. Relative project sidecars directory
        let search_dirs = [
            PathBuf::from("sidecars"),
            PathBuf::from("dist-bin").join("sidecars"),
            PathBuf::from("../../sidecars"),
        ];

        for dir in &search_dirs {
            let direct = dir.join(bin_name);
            if direct.is_file() {
                return Some(direct);
            }
            if let Some(target) = target_triple {
                let target_p = dir.join(target).join(bin_name);
                if target_p.is_file() {
                    return Some(target_p);
                }
            }
        }

        // 4. Executable adjacent directory
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let adj = exe_dir.join("sidecars").join(bin_name);
                if adj.is_file() {
                    return Some(adj);
                }
            }
        }

        // 5. System PATH lookup
        which_lookup(bin_name)
    }
}

fn which_lookup(bin_name: &str) -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            let candidate = PathBuf::from(dir).join(bin_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn chrono_timestamp() -> String {
    // Return standard ISO 8601 timestamp without requiring chrono dependency
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}.{:03}Z", now.as_secs(), now.subsec_millis())
}
