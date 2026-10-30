use std::path::{Path, PathBuf};
use tracing::warn;

pub struct PathQuarantine;

impl PathQuarantine {
    /// Returns true if the provided path is an OS-critical system path that must
    /// be quarantined to prevent accidental host system corruption or bricking.
    pub fn is_critical_system_path(path: &Path) -> bool {
        let norm_str = path.to_string_lossy().to_lowercase().replace('\\', "/");
        let clean = norm_str.trim_end_matches('/');

        // 1. Windows critical paths
        #[cfg(windows)]
        {
            let win_critical = [
                "c:/windows",
                "c:/recovery",
                "c:/$recycle.bin",
                "c:/system volume information",
                "c:/boot",
                "c:/bootmgr",
                "c:/pagefile.sys",
                "c:/hiberfil.sys",
                "c:/swapfile.sys",
                "c:/dumpstack.log.tmp",
            ];

            for crit in win_critical {
                if clean == crit || clean.starts_with(&format!("{}/", crit)) {
                    warn!("Quarantine intercept: Blocked access to Windows critical path {:?}", path);
                    return true;
                }
            }

            // Also check environment-defined Windows root (e.g. %WINDIR%, %SystemRoot%)
            if let Ok(windir) = std::env::var("WINDIR") {
                let windir_norm = windir.to_lowercase().replace('\\', "/");
                let windir_clean = windir_norm.trim_end_matches('/');
                if clean == windir_clean || clean.starts_with(&format!("{}/", windir_clean)) {
                    warn!("Quarantine intercept: Blocked access to %WINDIR% {:?}", path);
                    return true;
                }
            }
        }

        // 2. Unix / Linux / macOS critical paths
        let unix_critical = [
            "/proc",
            "/sys",
            "/etc",
            "/dev",
            "/boot",
            "/root",
            "/system",
            "/private",
            "/run",
            "/var/run",
            "/bin",
            "/sbin",
            "/usr/bin",
            "/usr/sbin",
            "/cores",
            "/library/apple",
        ];

        for crit in unix_critical {
            if clean == crit || clean.starts_with(&format!("{}/", crit)) {
                warn!("Quarantine intercept: Blocked access to Unix/POSIX critical path {:?}", path);
                return true;
            }
        }

        false
    }

    /// Checks if a file or directory name matches an OS-critical quarantine pattern
    /// when listing directory contents on a root or system volume.
    pub fn is_quarantined_name(name: &str) -> bool {
        let lower = name.to_lowercase();
        matches!(
            lower.as_str(),
            "windows"
                | "$recycle.bin"
                | "system volume information"
                | "recovery"
                | "boot"
                | "bootmgr"
                | "pagefile.sys"
                | "hiberfil.sys"
                | "swapfile.sys"
                | "dumpstack.log.tmp"
                | "proc"
                | "sys"
                | "etc"
                | "dev"
                | "cores"
        )
    }

    /// Determines if an entry within a parent directory should be excluded from raw unmanaged listing.
    pub fn can_expose_in_unmanaged_listing(_parent_path: &Path, entry_name: &str, entry_path: &Path) -> bool {
        // Exclude if entry name matches quarantined OS-critical name
        if Self::is_quarantined_name(entry_name) {
            return false;
        }

        // Always quarantine OS-critical paths directly
        if Self::is_critical_system_path(entry_path) {
            return false;
        }

        true
    }
}
