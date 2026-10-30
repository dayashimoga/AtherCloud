use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use sysinfo::Disks;
use tracing::{debug, info};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiskMediaType {
    NvmeSsd,
    SataSsd,
    RotationalHdd,
    RemovableUsb,
    OpticalDrive,
    NetworkDrive,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub name: String,
    pub mount_point: PathBuf,
    pub filesystem: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub is_removable: bool,
    pub is_system: bool,
    pub media_type: DiskMediaType,
    pub serial_number: Option<String>,
    pub is_read_only: bool,
}

pub struct VolumeInspector;

impl VolumeInspector {
    /// Inspects and enumerates all mounted physical and logical host volumes
    /// using `sysinfo` unified disk telemetry combined with OS native heuristics.
    pub fn inspect_volumes() -> Vec<VolumeInfo> {
        let mut volumes = Vec::new();

        if let Ok(disks) = Disks::new_with_refreshed_list() {
            for disk in disks.iter() {
                let mount_point = disk.mount_point().to_path_buf();
                let fs_name = disk.file_system().to_string_lossy().to_string();
                let total_bytes = disk.total_space();
                let available_bytes = disk.available_space();
                let is_read_only = disk.is_read_only();

                let (name, is_removable, is_system, media_type, serial_number) =
                    Self::inspect_os_volume_details(&mount_point, disk.is_removable());

                let info = VolumeInfo {
                    name,
                    mount_point,
                    filesystem: if fs_name.is_empty() { "Unknown".to_string() } else { fs_name },
                    total_bytes,
                    available_bytes,
                    is_removable,
                    is_system,
                    media_type,
                    serial_number,
                    is_read_only,
                };

                debug!("Inspected Volume: {:?}", info);
                volumes.push(info);
            }
        }

        // Fallback for Windows in case sysinfo returns an empty set
        #[cfg(windows)]
        if volumes.is_empty() {
            volumes = Self::inspect_windows_drives_fallback();
        }

        info!("VolumeInspector detected {} active host storage volume(s)", volumes.len());
        volumes
    }

    #[cfg(windows)]
    fn inspect_os_volume_details(mount_point: &Path, sysinfo_removable: bool) -> (String, bool, bool, DiskMediaType, Option<String>) {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        let path_str = mount_point.to_string_lossy().to_uppercase();
        let is_system = path_str.starts_with("C:") || path_str == "C:\\";

        // Convert path to wide null-terminated string for Windows API
        let root_str = if path_str.ends_with('\\') {
            path_str.clone()
        } else {
            format!("{}\\", path_str)
        };
        let wide: Vec<u16> = OsStr::new(&root_str).encode_wide().chain(Some(0)).collect();

        // Windows Win32 Drive Type constants
        const DRIVE_REMOVABLE: u32 = 2;
        const DRIVE_FIXED: u32 = 3;
        const DRIVE_REMOTE: u32 = 4;
        const DRIVE_CDROM: u32 = 5;

        // 1. GetDriveTypeW
        let drive_type_raw = unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(wide.as_ptr()) };
        let is_removable = sysinfo_removable
            || drive_type_raw == DRIVE_REMOVABLE
            || drive_type_raw == DRIVE_CDROM;

        let media_type = match drive_type_raw {
            DRIVE_REMOVABLE => DiskMediaType::RemovableUsb,
            DRIVE_CDROM => DiskMediaType::OpticalDrive,
            DRIVE_REMOTE => DiskMediaType::NetworkDrive,
            DRIVE_FIXED => {
                if is_system {
                    DiskMediaType::NvmeSsd
                } else {
                    DiskMediaType::SataSsd
                }
            }
            _ => DiskMediaType::Unknown,
        };

        // 2. GetVolumeInformationW
        let mut volume_name_buf = [0u16; 261];
        let mut serial_number: u32 = 0;
        let mut max_component_len: u32 = 0;
        let mut flags: u32 = 0;
        let mut fs_name_buf = [0u16; 261];

        let success = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW(
                wide.as_ptr(),
                volume_name_buf.as_mut_ptr(),
                volume_name_buf.len() as u32,
                &mut serial_number,
                &mut max_component_len,
                &mut flags,
                fs_name_buf.as_mut_ptr(),
                fs_name_buf.len() as u32,
            )
        };

        let serial_str = if success != 0 && serial_number != 0 {
            Some(format!("{:04X}-{:04X}", (serial_number >> 16) & 0xFFFF, serial_number & 0xFFFF))
        } else {
            None
        };

        let label = if success != 0 {
            let end = volume_name_buf.iter().position(|&c| c == 0).unwrap_or(volume_name_buf.len());
            String::from_utf16_lossy(&volume_name_buf[..end])
        } else {
            String::new()
        };

        let drive_letter = root_str.chars().next().unwrap_or('?');
        let display_name = if !label.trim().is_empty() {
            format!("{} ({}:)", label.trim(), drive_letter)
        } else {
            format!("Local Disk ({}:)", drive_letter)
        };

        (display_name, is_removable, is_system, media_type, serial_str)
    }

    #[cfg(not(windows))]
    fn inspect_os_volume_details(mount_point: &Path, sysinfo_removable: bool) -> (String, bool, bool, DiskMediaType, Option<String>) {
        let path_str = mount_point.to_string_lossy();
        let is_system = path_str == "/" || path_str == "/System";

        let media_type = if sysinfo_removable || path_str.contains("/media") || path_str.contains("/Volumes") {
            DiskMediaType::RemovableUsb
        } else if is_system {
            DiskMediaType::NvmeSsd
        } else {
            DiskMediaType::SataSsd
        };

        let display_name = if is_system {
            "System Root (/)".to_string()
        } else {
            mount_point
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path_str.to_string())
        };

        (display_name, sysinfo_removable, is_system, media_type, None)
    }

    #[cfg(windows)]
    fn inspect_windows_drives_fallback() -> Vec<VolumeInfo> {
        let mut list = Vec::new();
        let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };

        for i in 0..26 {
            if (mask & (1 << i)) != 0 {
                let letter = (b'A' + i as u8) as char;
                let root_str = format!("{}:\\", letter);
                let mount_point = PathBuf::from(&root_str);

                let (name, is_removable, is_system, media_type, serial_number) =
                    Self::inspect_os_volume_details(&mount_point, false);

                let mut free_bytes_avail: u64 = 0;
                let mut total_number_bytes: u64 = 0;
                let mut total_number_free_bytes: u64 = 0;

                use std::ffi::OsStr;
                use std::os::windows::ffi::OsStrExt;
                let wide: Vec<u16> = OsStr::new(&root_str).encode_wide().chain(Some(0)).collect();

                unsafe {
                    windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                        wide.as_ptr(),
                        &mut free_bytes_avail,
                        &mut total_number_bytes,
                        &mut total_number_free_bytes,
                    );
                }

                list.push(VolumeInfo {
                    name,
                    mount_point,
                    filesystem: "NTFS".to_string(),
                    total_bytes: total_number_bytes,
                    available_bytes: free_bytes_avail,
                    is_removable,
                    is_system,
                    media_type,
                    serial_number,
                    is_read_only: false,
                });
            }
        }
        list
    }
}
