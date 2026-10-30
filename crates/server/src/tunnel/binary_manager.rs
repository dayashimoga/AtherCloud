use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum BinaryError {
    #[error("Platform unsupported: {0}")]
    UnsupportedPlatform(String),
    #[error("Binary not found in sidecar, PATH, or configured paths")]
    NotFound,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Download error: {0}")]
    Download(String),
    #[error("Checksum mismatch: expected {expected}, actual {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Execution timeout probing binary version")]
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlatformArch {
    WindowsAmd64,
    Windows386,
    WindowsArm64,
    LinuxAmd64,
    LinuxArm64,
    LinuxArm,
    DarwinAmd64,
    DarwinArm64,
    Unknown(String),
}

impl PlatformArch {
    pub fn detect() -> Self {
        Self::from_os_arch(std::env::consts::OS, std::env::consts::ARCH)
    }

    pub fn from_os_arch(os: &str, arch: &str) -> Self {
        match (os, arch) {
            ("windows", "x86_64") => PlatformArch::WindowsAmd64,
            ("windows", "x86") => PlatformArch::Windows386,
            ("windows", "aarch64") => PlatformArch::WindowsArm64,
            ("linux", "x86_64") => PlatformArch::LinuxAmd64,
            ("linux", "aarch64") => PlatformArch::LinuxArm64,
            ("linux", "arm") => PlatformArch::LinuxArm,
            ("macos", "x86_64") => PlatformArch::DarwinAmd64,
            ("macos", "aarch64") => PlatformArch::DarwinArm64,
            (other_os, other_arch) => PlatformArch::Unknown(format!("{other_os}-{other_arch}")),
        }
    }

    pub fn is_supported(&self) -> bool {
        !matches!(self, PlatformArch::Unknown(_))
    }

    pub fn binary_filename(&self) -> &'static str {
        match self {
            PlatformArch::WindowsAmd64 | PlatformArch::Windows386 | PlatformArch::WindowsArm64 => {
                "cloudflared.exe"
            }
            _ => "cloudflared",
        }
    }

    pub fn remote_artifact_name(&self) -> Option<&'static str> {
        match self {
            PlatformArch::WindowsAmd64 => Some("cloudflared-windows-amd64.exe"),
            PlatformArch::Windows386 => Some("cloudflared-windows-386.exe"),
            PlatformArch::WindowsArm64 => Some("cloudflared-windows-arm64.exe"),
            PlatformArch::LinuxAmd64 => Some("cloudflared-linux-amd64"),
            PlatformArch::LinuxArm64 => Some("cloudflared-linux-arm64"),
            PlatformArch::LinuxArm => Some("cloudflared-linux-arm"),
            PlatformArch::DarwinAmd64 => Some("cloudflared-darwin-amd64.tgz"),
            PlatformArch::DarwinArm64 => Some("cloudflared-darwin-arm64.tgz"),
            PlatformArch::Unknown(_) => None,
        }
    }

    pub fn download_url(&self, version: Option<&str>) -> Option<String> {
        let artifact = self.remote_artifact_name()?;
        match version {
            Some(v) => Some(format!(
                "https://github.com/cloudflare/cloudflared/releases/download/{v}/{artifact}"
            )),
            None => Some(format!(
                "https://github.com/cloudflare/cloudflared/releases/latest/download/{artifact}"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinarySource {
    CustomPath,
    LocalSidecar,
    SystemPath,
    AutoDownloaded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflaredBinaryInfo {
    pub path: PathBuf,
    pub source: BinarySource,
    pub platform: PlatformArch,
    pub version: Option<String>,
    pub sha256: Option<String>,
    pub file_size: u64,
    pub is_executable: bool,
}

#[derive(Debug, Clone)]
pub struct BinaryManagerConfig {
    pub sidecar_dir: PathBuf,
    pub custom_path: Option<PathBuf>,
    pub download_url_override: Option<String>,
    pub expected_sha256: Option<String>,
    pub allow_auto_download: bool,
    pub timeout: Duration,
}

impl Default for BinaryManagerConfig {
    fn default() -> Self {
        Self {
            sidecar_dir: PathBuf::from("bin"),
            custom_path: None,
            download_url_override: None,
            expected_sha256: None,
            allow_auto_download: true,
            timeout: Duration::from_secs(5),
        }
    }
}

pub struct CloudflaredBinaryManager {
    config: BinaryManagerConfig,
    platform: PlatformArch,
}

impl CloudflaredBinaryManager {
    pub fn new(config: BinaryManagerConfig) -> Self {
        let platform = PlatformArch::detect();
        Self { config, platform }
    }

    pub fn with_platform(mut self, platform: PlatformArch) -> Self {
        self.platform = platform;
        self
    }

    pub fn platform(&self) -> &PlatformArch {
        &self.platform
    }

    pub fn config(&self) -> &BinaryManagerConfig {
        &self.config
    }

    pub fn find_system_binary(&self) -> Option<PathBuf> {
        find_in_system_path(self.platform.binary_filename())
    }

    pub fn find_sidecar_binary(&self) -> Option<PathBuf> {
        let sidecar_bin = self.config.sidecar_dir.join(self.platform.binary_filename());
        if sidecar_bin.is_file() {
            Some(sidecar_bin)
        } else {
            None
        }
    }

    pub async fn probe_version(&self, binary_path: &Path) -> Result<String, BinaryError> {
        probe_version(binary_path, self.config.timeout).await
    }

    pub fn compute_sha256(&self, path: &Path) -> Result<String, BinaryError> {
        compute_file_sha256(path).map_err(BinaryError::Io)
    }

    pub async fn download_binary(
        &self,
        target_path: &Path,
        url: &str,
        expected_sha256: Option<&str>,
    ) -> Result<(), BinaryError> {
        download_binary(url, target_path, expected_sha256).await
    }

    pub fn mock_install_binary(
        &self,
        name: &str,
        content: &[u8],
    ) -> Result<PathBuf, BinaryError> {
        let target = self.config.sidecar_dir.join(name);
        mock_install_binary(&target, content)
    }

    pub async fn resolve(&self) -> Result<CloudflaredBinaryInfo, BinaryError> {
        // 1. Custom Path
        if let Some(ref custom_path) = self.config.custom_path {
            if custom_path.is_file() {
                return self.inspect_binary(custom_path, BinarySource::CustomPath).await;
            }
        }

        let bin_name = self.platform.binary_filename();

        // 2. Local Sidecar
        let sidecar_bin = self.config.sidecar_dir.join(bin_name);
        if sidecar_bin.is_file() {
            return self.inspect_binary(&sidecar_bin, BinarySource::LocalSidecar).await;
        }

        // 3. System PATH
        if let Some(sys_bin) = find_in_system_path(bin_name) {
            if sys_bin.is_file() {
                return self.inspect_binary(&sys_bin, BinarySource::SystemPath).await;
            }
        }

        // 4. Auto-download if permitted
        if self.config.allow_auto_download {
            let download_url = if let Some(ref override_url) = self.config.download_url_override {
                Some(override_url.clone())
            } else {
                self.platform.download_url(None)
            };

            if let Some(url) = download_url {
                let target = self.config.sidecar_dir.join(bin_name);
                download_binary(&url, &target, self.config.expected_sha256.as_deref()).await?;
                return self.inspect_binary(&target, BinarySource::AutoDownloaded).await;
            } else {
                return Err(BinaryError::UnsupportedPlatform(format!(
                    "{:?} does not have a standard download URL",
                    self.platform
                )));
            }
        }

        Err(BinaryError::NotFound)
    }

    pub async fn inspect_binary(
        &self,
        path: &Path,
        source: BinarySource,
    ) -> Result<CloudflaredBinaryInfo, BinaryError> {
        let metadata = std::fs::metadata(path).map_err(BinaryError::Io)?;
        let file_size = metadata.len();

        #[cfg(unix)]
        let is_executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        };

        #[cfg(not(unix))]
        let is_executable = {
            path.extension()
                .map(|ext| {
                    ext.eq_ignore_ascii_case("exe")
                        || ext.eq_ignore_ascii_case("cmd")
                        || ext.eq_ignore_ascii_case("bat")
                })
                .unwrap_or(false)
                || metadata.is_file()
        };

        let sha256 = compute_file_sha256(path).ok();
        let version = probe_version(path, self.config.timeout).await.ok();

        Ok(CloudflaredBinaryInfo {
            path: path.to_path_buf(),
            source,
            platform: self.platform.clone(),
            version,
            sha256,
            file_size,
            is_executable,
        })
    }
}

pub fn parse_version_string(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(pos) = line.find("version ") {
            let after = &line[pos + 8..];
            let ver = after.split_whitespace().next().unwrap_or("").trim();
            if !ver.is_empty() {
                return Some(ver.to_string());
            }
        }
    }
    output.lines().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string())
}

pub fn compute_file_sha256(path: &Path) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn find_in_system_path(binary_name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH").or_else(|| std::env::var_os("Path"))?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary_name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            if !binary_name.to_lowercase().ends_with(".exe") {
                let candidate_exe = dir.join(format!("{binary_name}.exe"));
                if candidate_exe.is_file() {
                    return Some(candidate_exe);
                }
            }
        }
    }
    None
}

pub async fn probe_version(
    binary_path: &Path,
    timeout_duration: Duration,
) -> Result<String, BinaryError> {
    let mut cmd = tokio::process::Command::new(binary_path);
    cmd.arg("--version");

    let child = cmd.output();
    let output = match tokio::time::timeout(timeout_duration, child).await {
        Ok(res) => res.map_err(|e| BinaryError::ExecutionFailed(e.to_string()))?,
        Err(_) => return Err(BinaryError::Timeout),
    };

    let out_str = String::from_utf8_lossy(&output.stdout);
    let err_str = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{out_str}\n{err_str}");

    if let Some(ver) = parse_version_string(&combined) {
        Ok(ver)
    } else if !output.status.success() {
        Err(BinaryError::ExecutionFailed(format!(
            "exit status: {}, stderr: {}",
            output.status, err_str
        )))
    } else {
        Err(BinaryError::ExecutionFailed(
            "Could not parse version from output".to_string(),
        ))
    }
}

pub async fn download_binary(
    url: &str,
    target_path: &Path,
    expected_sha256: Option<&str>,
) -> Result<(), BinaryError> {
    if let Some(parent) = target_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(BinaryError::Io)?;
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| BinaryError::Download(e.to_string()))?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| BinaryError::Download(e.to_string()))?;

    if !response.status().is_success() {
        return Err(BinaryError::Download(format!(
            "HTTP error {} fetching from {}",
            response.status(),
            url
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| BinaryError::Download(e.to_string()))?;

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual_sha256 = hex::encode(hasher.finalize());

    if let Some(expected) = expected_sha256 {
        if !expected.eq_ignore_ascii_case(&actual_sha256) {
            return Err(BinaryError::ChecksumMismatch {
                expected: expected.to_string(),
                actual: actual_sha256,
            });
        }
    }

    let temp_path = target_path.with_extension(format!("tmp.{}", std::process::id()));
    tokio::fs::write(&temp_path, &bytes)
        .await
        .map_err(BinaryError::Io)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(&temp_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&temp_path, perms);
        }
    }

    if target_path.exists() {
        let _ = tokio::fs::remove_file(target_path).await;
    }

    tokio::fs::rename(&temp_path, target_path)
        .await
        .map_err(BinaryError::Io)?;

    Ok(())
}

pub fn mock_install_binary(dest_path: &Path, content: &[u8]) -> Result<PathBuf, BinaryError> {
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent).map_err(BinaryError::Io)?;
    }
    std::fs::write(dest_path, content).map_err(BinaryError::Io)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(dest_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(dest_path, perms);
        }
    }

    Ok(dest_path.to_path_buf())
}
