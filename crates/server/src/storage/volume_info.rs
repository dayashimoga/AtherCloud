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
    pub filesystem_label: Option<String>,
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

                let (name, label, is_removable, is_system, media_type, serial_number) =
                    Self::inspect_os_volume_details(&mount_point, disk.is_removable());

                let info = VolumeInfo {
                    name,
                    mount_point,
                    filesystem: if fs_name.is_empty() { "Unknown".to_string() } else { fs_name },
                    filesystem_label: label,
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
    fn inspect_os_volume_details(
        mount_point: &Path,
        sysinfo_removable: bool,
    ) -> (String, Option<String>, bool, bool, DiskMediaType, Option<String>) {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        let path_str = mount_point.to_string_lossy().to_uppercase();
        let is_system = path_str.starts_with("C:") || path_str == "C:\\";
        let drive_letter = path_str.chars().next().unwrap_or('C');

        // Convert path to wide null-terminated string for Windows API
        let root_str = if path_str.ends_with('\\') {
            path_str.clone()
        } else {
            format!("{}\\", path_str)
        };
        let wide: Vec<u16> = OsStr::new(&root_str).encode_wide().chain(Some(0)).collect();

        // 1. Query Win32 Volume Information (Label, Serial, FS)
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

        let volume_serial = if success != 0 && serial_number != 0 {
            Some(format!("{:04X}-{:04X}", (serial_number >> 16) & 0xFFFF, serial_number & 0xFFFF))
        } else {
            None
        };

        let label_opt = if success != 0 {
            let end = volume_name_buf.iter().position(|&c| c == 0).unwrap_or(volume_name_buf.len());
            let raw_label = String::from_utf16_lossy(&volume_name_buf[..end]).trim().to_string();
            if raw_label.is_empty() {
                None
            } else {
                Some(raw_label)
            }
        } else {
            None
        };

        // 2. Query Windows Physical Storage Bus Properties (NVMe, SATA, USB)
        let (bus_media_type, hw_serial) = Self::query_windows_storage_device_property(drive_letter);

        // 3. Fallback to GetDriveTypeW
        const DRIVE_REMOVABLE: u32 = 2;
        const DRIVE_FIXED: u32 = 3;
        const DRIVE_REMOTE: u32 = 4;
        const DRIVE_CDROM: u32 = 5;

        let drive_type_raw = unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(wide.as_ptr()) };
        let is_removable = sysinfo_removable
            || drive_type_raw == DRIVE_REMOVABLE
            || drive_type_raw == DRIVE_CDROM
            || bus_media_type == DiskMediaType::RemovableUsb;

        let media_type = if bus_media_type != DiskMediaType::Unknown {
            bus_media_type
        } else {
            match drive_type_raw {
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
            }
        };

        let final_serial = hw_serial.or(volume_serial);

        let display_name = match &label_opt {
            Some(lbl) => format!("{} ({}:)", lbl, drive_letter),
            None => format!("Local Disk ({}:)", drive_letter),
        };

        (display_name, label_opt, is_removable, is_system, media_type, final_serial)
    }

    #[cfg(windows)]
    fn query_windows_storage_device_property(drive_letter: char) -> (DiskMediaType, Option<String>) {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        let dev_path = format!(r"\\.\{}:", drive_letter);
        let wide: Vec<u16> = OsStr::new(&dev_path).encode_wide().chain(Some(0)).collect();

        const FILE_SHARE_READ: u32 = 1;
        const FILE_SHARE_WRITE: u32 = 2;
        const OPEN_EXISTING: u32 = 3;
        const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D1400;

        let handle = unsafe {
            windows_sys::Win32::Storage::FileSystem::CreateFileW(
                wide.as_ptr(),
                0, // 0 access rights allows querying device properties without admin elevation
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                0,
            )
        };

        if handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return (DiskMediaType::Unknown, None);
        }

        #[repr(C)]
        struct StoragePropertyQuery {
            property_id: u32,
            query_type: u32,
            additional_parameters: [u8; 1],
        }

        let query = StoragePropertyQuery {
            property_id: 0, // StorageDeviceProperty
            query_type: 0,  // PropertyStandardQuery
            additional_parameters: [0],
        };

        let mut out_buf = [0u8; 1024];
        let mut bytes_returned: u32 = 0;

        let ok = unsafe {
            windows_sys::Win32::System::IO::DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &query as *const _ as *const _,
                std::mem::size_of::<StoragePropertyQuery>() as u32,
                out_buf.as_mut_ptr() as *mut _,
                out_buf.len() as u32,
                &mut bytes_returned,
                std::ptr::null_mut(),
            )
        };

        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(handle);
        }

        if ok != 0 && bytes_returned >= 28 {
            // BusType is at offset 24 (u32)
            // SerialNumberOffset is at offset 20 (u32)
            let bus_type = u32::from_ne_bytes(out_buf[24..28].try_into().unwrap_or_default());
            let serial_offset = u32::from_ne_bytes(out_buf[20..24].try_into().unwrap_or_default()) as usize;

            let media_type = match bus_type {
                17 => DiskMediaType::NvmeSsd,
                7 | 12 | 13 => DiskMediaType::RemovableUsb,
                11 | 3 => DiskMediaType::SataSsd,
                2 => DiskMediaType::OpticalDrive,
                9 => DiskMediaType::NetworkDrive,
                _ => DiskMediaType::Unknown,
            };

            let serial_str = if serial_offset > 0 && serial_offset < bytes_returned as usize {
                let slice = &out_buf[serial_offset..bytes_returned as usize];
                let len = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
                let clean = String::from_utf8_lossy(&slice[..len]).trim().to_string();
                if clean.is_empty() {
                    None
                } else {
                    Some(clean)
                }
            } else {
                None
            };

            (media_type, serial_str)
        } else {
            (DiskMediaType::Unknown, None)
        }
    }

    #[cfg(not(windows))]
    fn inspect_os_volume_details(
        mount_point: &Path,
        sysinfo_removable: bool,
    ) -> (String, Option<String>, bool, bool, DiskMediaType, Option<String>) {
        let path_str = mount_point.to_string_lossy();
        let is_system = path_str == "/" || path_str == "/System";

        // Query Linux /sys/block rotational & removable attributes if present
        let mut media_type = if sysinfo_removable || path_str.contains("/media") || path_str.contains("/Volumes") {
            DiskMediaType::RemovableUsb
        } else if is_system {
            DiskMediaType::NvmeSsd
        } else {
            DiskMediaType::SataSsd
        };

        let mut serial_number: Option<String> = None;
        let mut label_opt: Option<String> = None;

        #[cfg(target_os = "linux")]
        {
            if let Ok(entries) = std::fs::read_dir("/sys/block") {
                for entry in entries.flatten() {
                    let dev_name = entry.file_name().to_string_lossy().to_string();
                    if dev_name.starts_with("loop") || dev_name.starts_with("ram") {
                        continue;
                    }
                    if dev_name.starts_with("sr") {
                        media_type = DiskMediaType::OpticalDrive;
                    } else if dev_name.starts_with("nvme") {
                        media_type = DiskMediaType::NvmeSsd;
                    } else {
                        let rotational_path = entry.path().join("queue/rotational");
                        if let Ok(val) = std::fs::read_to_string(rotational_path) {
                            if val.trim() == "1" {
                                media_type = DiskMediaType::RotationalHdd;
                            } else if val.trim() == "0" {
                                media_type = DiskMediaType::SataSsd;
                            }
                        }
                    }
                    break;
                }
            }

            // Check /dev/disk/by-label for volume label
            if let Ok(entries) = std::fs::read_dir("/dev/disk/by-label") {
                for entry in entries.flatten() {
                    label_opt = Some(entry.file_name().to_string_lossy().to_string());
                    break;
                }
            }
        }

        let display_name = if is_system {
            "System Root (/)".to_string()
        } else if let Some(ref lbl) = label_opt {
            lbl.clone()
        } else {
            mount_point
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path_str.to_string())
        };

        (display_name, label_opt, sysinfo_removable, is_system, media_type, serial_number)
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

                let (name, label, is_removable, is_system, media_type, serial_number) =
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
                    filesystem_label: label,
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
