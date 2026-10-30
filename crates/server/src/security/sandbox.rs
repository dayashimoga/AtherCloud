use std::path::{Component, Path, PathBuf};
use tracing::{error, warn};

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("Security violation: Null byte injection detected in path '{0}'")]
    NullByteInjection(String),
    #[error("Security violation: Directory traversal attempt detected: '{0}'")]
    DirectoryTraversal(String),
    #[error("Security violation: Symlink escape detected: target '{0}' escapes sandbox root '{1}'")]
    SymlinkEscape(PathBuf, PathBuf),
    #[error("Security violation: Windows device or UNC path bypass rejected: '{0}'")]
    UncDeviceBypass(String),
    #[error("Security violation: Forbidden DOS reserved device name: '{0}'")]
    ReservedDeviceName(String),
    #[error("Target not found: '{0}'")]
    NotFound(PathBuf),
    #[error("I/O error during path canonicalization: {0}")]
    Io(#[from] std::io::Error),
    #[error("Security violation: OS-critical system path is quarantined: '{0}'")]
    QuarantinedPath(PathBuf),
    #[error("Security violation: Requested subpath '{0}' is not in mount's allowed whitelist")]
    SubpathForbidden(PathBuf),
}

pub struct PathSandbox;

impl PathSandbox {
    /// Strict path resolution using `dunce::canonicalize` preventing:
    /// - Null byte injection (`\0`, `%00`)
    /// - Directory traversal (`../`, `..\`, `%2e%2e`)
    /// - Symlink escapes resolving outside mount root
    /// - Windows UNC path bypasses (`\\server\share`, `//server/share`)
    /// - Windows device namespace escapes (`\\.\`, `\\?\`, `\\.\GlobalRoot`)
    /// - DOS reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`..`COM9`, `LPT1`..`LPT9`)
    pub fn resolve_sandboxed_path(
        base_root: &Path,
        requested_subpath: &Path,
    ) -> Result<PathBuf, SandboxError> {
        let subpath_str = requested_subpath.to_string_lossy();

        // 1. Check for null byte injection (raw byte or URI encoded)
        if subpath_str.contains('\0') || subpath_str.contains("%00") || subpath_str.contains("\\0") {
            error!("Null byte injection attempt blocked: '{}'", subpath_str);
            return Err(SandboxError::NullByteInjection(subpath_str.into_owned()));
        }

        // 2. Reject Windows UNC network paths and NT device namespaces
        if Self::is_unc_or_device_namespace(&subpath_str) {
            error!("Windows device or UNC bypass blocked: '{}'", subpath_str);
            return Err(SandboxError::UncDeviceBypass(subpath_str.into_owned()));
        }

        // 3. Inspect raw string for URL-encoded traversal sequences (%2e%2e, %2f, %5c)
        let lower = subpath_str.to_lowercase();
        if lower.contains("%2e%2e")
            || lower.contains("%2e.")
            || lower.contains(".%2e")
            || lower.contains("..%2f")
            || lower.contains("..%5c")
        {
            error!("Encoded directory traversal attempt blocked: '{}'", subpath_str);
            return Err(SandboxError::DirectoryTraversal(subpath_str.into_owned()));
        }

        // 4. Sanitize and validate path components
        for comp in requested_subpath.components() {
            match comp {
                Component::ParentDir => {
                    error!("Parent traversal ('..') blocked: '{}'", subpath_str);
                    return Err(SandboxError::DirectoryTraversal(subpath_str.into_owned()));
                }
                Component::RootDir | Component::Prefix(_) => {
                    error!("Absolute root or drive prefix blocked: '{}'", subpath_str);
                    return Err(SandboxError::DirectoryTraversal(subpath_str.into_owned()));
                }
                Component::Normal(os_str) => {
                    let comp_str = os_str.to_string_lossy();
                    if Self::is_dos_device_name(&comp_str) {
                        error!("Forbidden DOS reserved device name blocked: '{}'", comp_str);
                        return Err(SandboxError::ReservedDeviceName(comp_str.into_owned()));
                    }
                    if comp_str == ".." || comp_str == "." || comp_str.contains('/') || comp_str.contains('\\') {
                        return Err(SandboxError::DirectoryTraversal(subpath_str.into_owned()));
                    }
                }
                Component::CurDir => {}
            }
        }

        // 5. Canonicalize base root ensuring it exists and is not quarantined
        let canonical_base = dunce::canonicalize(base_root)?;
        if crate::storage::PathQuarantine::is_critical_system_path(&canonical_base) {
            return Err(SandboxError::QuarantinedPath(canonical_base));
        }

        let combined = canonical_base.join(requested_subpath);

        // 6. Strict symlink and boundary escape resolution
        if combined.exists() {
            let canonical_target = dunce::canonicalize(&combined)?;

            // Check if canonical target escapes the canonical base root
            if !canonical_target.starts_with(&canonical_base) {
                error!(
                    "Symlink escape detected: '{}' resolves to '{}' outside root '{}'",
                    combined.display(),
                    canonical_target.display(),
                    canonical_base.display()
                );
                return Err(SandboxError::SymlinkEscape(canonical_target, canonical_base));
            }

            // Verify canonical target is not pointing to an OS-quarantined location
            if crate::storage::PathQuarantine::is_critical_system_path(&canonical_target) {
                return Err(SandboxError::QuarantinedPath(canonical_target));
            }

            Ok(canonical_target)
        } else {
            // Target does not exist yet (e.g., target file for upload or creation)
            // Canonicalize closest existing parent directory to prevent symlinked parent directory escapes
            if let Some(parent) = combined.parent() {
                if parent.exists() {
                    let canonical_parent = dunce::canonicalize(parent)?;
                    if !canonical_parent.starts_with(&canonical_base) {
                        return Err(SandboxError::SymlinkEscape(canonical_parent, canonical_base));
                    }
                }
            }

            if !combined.starts_with(&canonical_base) {
                return Err(SandboxError::DirectoryTraversal(subpath_str.into_owned()));
            }

            Ok(combined)
        }
    }

    /// Normalizes subpath into relative components stripping leading root/prefixes
    pub fn normalize_subpath(path: &Path) -> PathBuf {
        let mut normalized = PathBuf::new();
        for comp in path.components() {
            match comp {
                Component::Normal(c) => normalized.push(c),
                Component::CurDir => {}
                _ => {}
            }
        }
        normalized
    }

    /// Checks if a prefix path matches component-by-component against target path
    pub fn is_component_prefix(target: &Path, prefix: &Path) -> bool {
        let target_comps: Vec<_> = target.components().collect();
        let prefix_comps: Vec<_> = prefix.components().collect();

        if prefix_comps.len() > target_comps.len() {
            return false;
        }

        for (i, p_comp) in prefix_comps.iter().enumerate() {
            let t_comp = &target_comps[i];
            #[cfg(windows)]
            {
                let p_str = p_comp.as_os_str().to_string_lossy().to_lowercase();
                let t_str = t_comp.as_os_str().to_string_lossy().to_lowercase();
                if p_str != t_str {
                    return false;
                }
            }
            #[cfg(not(windows))]
            {
                if p_comp != t_comp {
                    return false;
                }
            }
        }
        true
    }

    /// Intercepts and validates requested subpaths against the mount's `allowed_subpaths` whitelist
    /// BEFORE executing any file metadata or read syscall.
    ///
    /// - If `allowed_subpaths` is empty, access is unrestricted within the mount root.
    /// - If non-empty, the requested subpath must match or be a descendant of at least one whitelist entry.
    /// - Pure in-memory lexical verification without touching the host filesystem.
    pub fn validate_subpath_whitelist(
        allowed_subpaths: &[PathBuf],
        requested_subpath: &Path,
    ) -> Result<(), SandboxError> {
        let subpath_str = requested_subpath.to_string_lossy();

        // 1. Detect null byte injection
        if subpath_str.contains('\0') || subpath_str.contains("%00") || subpath_str.contains("\\0") {
            error!("Null byte injection attempt in whitelist check: '{}'", subpath_str);
            return Err(SandboxError::NullByteInjection(subpath_str.into_owned()));
        }

        // 2. Reject Windows UNC or device namespaces
        if Self::is_unc_or_device_namespace(&subpath_str) {
            error!("Windows UNC/device bypass in whitelist check: '{}'", subpath_str);
            return Err(SandboxError::UncDeviceBypass(subpath_str.into_owned()));
        }

        // 3. Inspect for traversal sequences
        let lower = subpath_str.to_lowercase();
        if lower.contains("%2e%2e")
            || lower.contains("%2e.")
            || lower.contains(".%2e")
            || lower.contains("..%2f")
            || lower.contains("..%5c")
        {
            error!("Encoded directory traversal in whitelist check: '{}'", subpath_str);
            return Err(SandboxError::DirectoryTraversal(subpath_str.into_owned()));
        }

        for comp in requested_subpath.components() {
            match comp {
                Component::ParentDir => {
                    error!("Parent traversal ('..') in whitelist check: '{}'", subpath_str);
                    return Err(SandboxError::DirectoryTraversal(subpath_str.into_owned()));
                }
                Component::RootDir | Component::Prefix(_) => {
                    error!("Absolute/prefix root in whitelist check: '{}'", subpath_str);
                    return Err(SandboxError::DirectoryTraversal(subpath_str.into_owned()));
                }
                Component::Normal(os_str) => {
                    let comp_str = os_str.to_string_lossy();
                    if Self::is_dos_device_name(&comp_str) {
                        return Err(SandboxError::ReservedDeviceName(comp_str.into_owned()));
                    }
                }
                Component::CurDir => {}
            }
        }

        // 4. If whitelist is empty, all subpaths within mount root are authorized
        if allowed_subpaths.is_empty() {
            return Ok(());
        }

        let req_norm = Self::normalize_subpath(requested_subpath);

        // 5. Match against allowed subpaths
        for allowed in allowed_subpaths {
            let allowed_norm = Self::normalize_subpath(allowed);
            if allowed_norm.as_os_str().is_empty() {
                return Ok(());
            }
            if Self::is_component_prefix(&req_norm, &allowed_norm) {
                return Ok(());
            }
        }

        warn!(
            "Subpath whitelist violation: requested subpath '{}' not in allowed whitelist {:?}",
            requested_subpath.display(),
            allowed_subpaths
        );
        Err(SandboxError::SubpathForbidden(requested_subpath.to_path_buf()))
    }

    /// Evaluates if a given subpath is an allowed subpath, descendant of an allowed subpath,
    /// or an ancestor folder leading to an allowed subpath (used for virtual folder hierarchy browsing).
    pub fn is_allowed_or_ancestor(allowed_subpaths: &[PathBuf], requested_subpath: &Path) -> bool {
        if allowed_subpaths.is_empty() {
            return true;
        }

        let subpath_str = requested_subpath.to_string_lossy();
        if subpath_str.contains('\0')
            || subpath_str.contains("%00")
            || subpath_str.contains("..")
            || Self::is_unc_or_device_namespace(&subpath_str)
        {
            return false;
        }

        let req_norm = Self::normalize_subpath(requested_subpath);
        for allowed in allowed_subpaths {
            let allowed_norm = Self::normalize_subpath(allowed);
            if allowed_norm.as_os_str().is_empty() {
                return true;
            }
            // Descendant or exact match
            if Self::is_component_prefix(&req_norm, &allowed_norm) {
                return true;
            }
            // Ancestor directory leading to allowed subpath
            if Self::is_component_prefix(&allowed_norm, &req_norm) {
                return true;
            }
        }
        false
    }

    /// Validates requested subpath against the mount whitelist and then strictly resolves
    /// the sandboxed canonical target path.
    pub fn resolve_sandboxed_path_with_whitelist(
        base_root: &Path,
        requested_subpath: &Path,
        allowed_subpaths: &[PathBuf],
    ) -> Result<PathBuf, SandboxError> {
        // Intercept and validate whitelist BEFORE executing any file metadata or read syscall
        Self::validate_subpath_whitelist(allowed_subpaths, requested_subpath)?;
        Self::resolve_sandboxed_path(base_root, requested_subpath)
    }

    /// Retrieves file metadata only after subpath whitelist verification and sandbox resolution.
    /// Intercepts and blocks the metadata syscall if the subpath is not whitelisted.
    pub fn sandboxed_metadata_with_whitelist(
        base_root: &Path,
        requested_subpath: &Path,
        allowed_subpaths: &[PathBuf],
    ) -> Result<std::fs::Metadata, SandboxError> {
        let safe_path = Self::resolve_sandboxed_path_with_whitelist(base_root, requested_subpath, allowed_subpaths)?;
        if !safe_path.exists() {
            return Err(SandboxError::NotFound(safe_path));
        }
        std::fs::metadata(&safe_path).map_err(SandboxError::Io)
    }

    /// Opens a file for reading only after subpath whitelist verification and sandbox resolution.
    /// Intercepts and blocks the open syscall if the subpath is not whitelisted.
    pub fn open_sandboxed_file_with_whitelist(
        base_root: &Path,
        requested_subpath: &Path,
        allowed_subpaths: &[PathBuf],
    ) -> Result<std::fs::File, SandboxError> {
        let safe_path = Self::resolve_sandboxed_path_with_whitelist(base_root, requested_subpath, allowed_subpaths)?;
        if !safe_path.exists() {
            return Err(SandboxError::NotFound(safe_path));
        }
        std::fs::File::open(&safe_path).map_err(SandboxError::Io)
    }

    /// Reads an entire file only after subpath whitelist verification and sandbox resolution.
    /// Intercepts and blocks the read syscall if the subpath is not whitelisted.
    pub fn read_sandboxed_file_with_whitelist(
        base_root: &Path,
        requested_subpath: &Path,
        allowed_subpaths: &[PathBuf],
    ) -> Result<Vec<u8>, SandboxError> {
        let safe_path = Self::resolve_sandboxed_path_with_whitelist(base_root, requested_subpath, allowed_subpaths)?;
        if !safe_path.exists() {
            return Err(SandboxError::NotFound(safe_path));
        }
        std::fs::read(&safe_path).map_err(SandboxError::Io)
    }

    /// Detects Windows UNC paths (e.g. `\\server\share`, `//server/share`) and
    /// NT object manager / device namespaces (`\\.\`, `\\?\`, `//./`, `//?/`)
    pub fn is_unc_or_device_namespace(path_str: &str) -> bool {
        let p = path_str.trim();
        // Starts with \\ or //
        if p.starts_with("\\\\") || p.starts_with("//") {
            return true;
        }
        // Device namespace checks: \??\, \\.\, \\?\, /??/, //./, //?/
        if p.starts_with("\\??\\")
            || p.starts_with("/??/")
            || p.contains("\\.\\")
            || p.contains("/./GlobalRoot")
            || p.to_lowercase().contains("globalroot")
            || p.to_lowercase().contains("physicaldrive")
        {
            return true;
        }
        false
    }

    /// Identifies legacy DOS 8.3 reserved device names (CON, PRN, AUX, NUL, COM1..9, LPT1..9)
    pub fn is_dos_device_name(name: &str) -> bool {
        let stem = match name.split('.').next() {
            Some(s) => s.trim(),
            None => name.trim(),
        };

        let upper = stem.to_ascii_uppercase();
        matches!(
            upper.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        )
    }
}
