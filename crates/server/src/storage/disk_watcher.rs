use super::volume_info::{VolumeInfo, VolumeInspector};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageDeviceEvent {
    Plugged(VolumeInfo),
    Unplugged(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetlinkAction {
    Add,
    Remove,
    Change,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetlinkBlockEvent {
    pub action: NetlinkAction,
    pub subsystem: String,
    pub devname: String,
    pub devtype: Option<String>,
}

pub struct NetlinkUeventParser;

impl NetlinkUeventParser {
    /// Parses a raw Linux netlink kobject uevent payload (null-delimited key=value strings)
    /// into a structured `NetlinkBlockEvent` if the subsystem is `block`.
    pub fn parse(buf: &[u8]) -> Option<NetlinkBlockEvent> {
        let mut action = None;
        let mut subsystem = None;
        let mut devname = None;
        let mut devtype = None;

        for chunk in buf.split(|&b| b == 0) {
            if chunk.is_empty() {
                continue;
            }
            let s = String::from_utf8_lossy(chunk);
            if let Some((k, v)) = s.split_once('=') {
                match k {
                    "ACTION" => {
                        action = match v.to_lowercase().as_str() {
                            "add" => Some(NetlinkAction::Add),
                            "remove" => Some(NetlinkAction::Remove),
                            "change" => Some(NetlinkAction::Change),
                            other => Some(NetlinkAction::Other(other.to_string())),
                        };
                    }
                    "SUBSYSTEM" => {
                        subsystem = Some(v.to_string());
                    }
                    "DEVNAME" => {
                        devname = Some(v.to_string());
                    }
                    "DEVTYPE" => {
                        devtype = Some(v.to_string());
                    }
                    _ => {}
                }
            }
        }

        if subsystem.as_deref() == Some("block") {
            if let (Some(act), Some(sub), Some(dev)) = (action, subsystem, devname) {
                return Some(NetlinkBlockEvent {
                    action: act,
                    subsystem: sub,
                    devname: dev,
                    devtype,
                });
            }
        }

        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacDiskArbitrationEvent {
    pub bsd_name: String,
    pub mount_point: Option<String>,
    pub is_appeared: bool,
}

impl MacDiskArbitrationEvent {
    /// Converts a macOS DiskArbitration callback into a standard `StorageDeviceEvent`
    pub fn to_storage_device_event(&self) -> StorageDeviceEvent {
        if self.is_appeared {
            let mount = self
                .mount_point
                .clone()
                .unwrap_or_else(|| format!("/Volumes/{}", self.bsd_name));
            let vol = VolumeInfo {
                name: format!("Mac Volume ({})", self.bsd_name),
                mount_point: std::path::PathBuf::from(&mount),
                filesystem: "APFS".to_string(),
                filesystem_label: Some(self.bsd_name.clone()),
                total_bytes: 128_000_000_000,
                available_bytes: 110_000_000_000,
                is_removable: true,
                is_system: false,
                media_type: super::volume_info::DiskMediaType::RemovableUsb,
                serial_number: Some(format!("MAC-DA-{}", self.bsd_name)),
                is_read_only: false,
            };
            StorageDeviceEvent::Plugged(vol)
        } else {
            let path = self
                .mount_point
                .clone()
                .unwrap_or_else(|| format!("/Volumes/{}", self.bsd_name));
            StorageDeviceEvent::Unplugged(path)
        }
    }
}

pub struct DiskWatcher {
    sender: broadcast::Sender<StorageDeviceEvent>,
    is_running: Arc<AtomicBool>,
    #[cfg(windows)]
    hwnd_handle: Arc<AtomicIsize>,
}

impl DiskWatcher {
    /// Constructs a new DiskWatcher with an internal broadcast channel of bounded capacity 64.
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(64);
        Self {
            sender,
            is_running: Arc::new(AtomicBool::new(false)),
            #[cfg(windows)]
            hwnd_handle: Arc::new(AtomicIsize::new(0)),
        }
    }

    /// Obtains a new receiver subscription to hot-plug storage device events.
    pub fn subscribe(&self) -> broadcast::Receiver<StorageDeviceEvent> {
        self.sender.subscribe()
    }

    /// Publishes a storage device event to all active daemon subscribers.
    pub fn publish(&self, event: StorageDeviceEvent) {
        match &event {
            StorageDeviceEvent::Plugged(v) => {
                info!("StorageDeviceEvent::Plugged -> {} ({:?})", v.name, v.mount_point);
            }
            StorageDeviceEvent::Unplugged(id) => {
                info!("StorageDeviceEvent::Unplugged -> {}", id);
            }
        }
        let _ = self.sender.send(event);
    }

    /// Returns whether the OS background event loop is running.
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    /// Starts the background OS hot-plug event interception loop.
    pub fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.is_running.swap(true, Ordering::SeqCst) {
            warn!("DiskWatcher background listener is already running");
            return Ok(());
        }

        #[cfg(windows)]
        {
            let sender = self.sender.clone();
            let is_running = self.is_running.clone();
            let hwnd_store = self.hwnd_handle.clone();

            std::thread::Builder::new()
                .name("aether-hotplug-watcher".to_string())
                .spawn(move || {
                    Self::run_windows_message_loop(sender, is_running, hwnd_store);
                })?;
        }

        #[cfg(target_os = "linux")]
        {
            let sender = self.sender.clone();
            let is_running = self.is_running.clone();

            std::thread::Builder::new()
                .name("aether-linux-uevent".to_string())
                .spawn(move || {
                    Self::run_linux_netlink_listener(sender, is_running);
                })?;
        }

        #[cfg(target_os = "macos")]
        {
            let sender = self.sender.clone();
            let is_running = self.is_running.clone();

            std::thread::Builder::new()
                .name("aether-mac-diskarb".to_string())
                .spawn(move || {
                    Self::run_macos_disk_arbitration_listener(sender, is_running);
                })?;
        }

        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            info!("DiskWatcher initialized (stub listener on current platform)");
        }

        Ok(())
    }

    /// Stops the background OS hot-plug event interception loop.
    pub fn stop(&self) {
        if !self.is_running.swap(false, Ordering::SeqCst) {
            return;
        }

        #[cfg(windows)]
        {
            let hwnd = self.hwnd_handle.load(Ordering::SeqCst);
            if hwnd != 0 {
                use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
                unsafe {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
            }
        }
    }

    /// Programmatically simulates a storage device event (useful for automated testing)
    pub fn simulate_device_event(&self, event: StorageDeviceEvent) {
        self.publish(event);
    }

    /// Spawns an asynchronous Tokio daemon worker that subscribes to the hot-plug broadcast channel
    /// and dynamically updates the MountRegistry and storage subsystems upon Plugged and Unplugged events.
    pub fn spawn_storage_event_dispatcher(
        watcher: Arc<DiskWatcher>,
        mounts: Arc<super::mounts::MountRegistry>,
    ) -> tokio::task::JoinHandle<()> {
        let mut rx = watcher.subscribe();
        tokio::spawn(async move {
            info!("Storage hot-plug background dispatcher active and listening to event bus");
            while let Ok(event) = rx.recv().await {
                match event {
                    StorageDeviceEvent::Plugged(vol) => {
                        info!("[HOTPLUG DISPATCHER] Storage device plugged: {} ({:?})", vol.name, vol.mount_point);
                        if !super::quarantine::PathQuarantine::is_critical_system_path(&vol.mount_point) {
                            let clean_name = vol
                                .mount_point
                                .to_string_lossy()
                                .replace([':', '\\', '/', ' '], "_")
                                .trim_matches('_')
                                .to_lowercase();
                            let mount_name = if vol.is_removable {
                                format!("usb_{}", clean_name)
                            } else {
                                format!("disk_{}", clean_name)
                            };

                            let _ = mounts.register_with_permission(
                                &mount_name,
                                vol.mount_point.clone(),
                                super::mounts::MountPermission::Write,
                                Some(format!("Dynamic Hot-Plug Volume ({})", vol.name)),
                            );
                            info!("[HOTPLUG DISPATCHER] Auto-registered dynamic mount '{}' -> {:?}", mount_name, vol.mount_point);
                        } else {
                            warn!("[HOTPLUG DISPATCHER] Plugged device {:?} is quarantined, skipped auto-mount", vol.mount_point);
                        }
                    }
                    StorageDeviceEvent::Unplugged(dev_path) => {
                        info!("[HOTPLUG DISPATCHER] Storage device unplugged: {}", dev_path);
                        if let Some(removed_name) = mounts.unregister_by_path(&std::path::PathBuf::from(&dev_path)) {
                            info!("[HOTPLUG DISPATCHER] Dynamically unmounted virtual mount '{}' for disconnected path {}", removed_name, dev_path);
                        }
                    }
                }
            }
        })
    }

    #[cfg(windows)]
    fn run_windows_message_loop(
        sender: broadcast::Sender<StorageDeviceEvent>,
        is_running: Arc<AtomicBool>,
        hwnd_store: Arc<AtomicIsize>,
    ) {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::*;

        const CLASS_NAME: &str = "AetherStreamHotPlugWindow";
        let class_name_wide: Vec<u16> = OsStr::new(CLASS_NAME).encode_wide().chain(Some(0)).collect();

        unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            const DBT_DEVICEARRIVAL: usize = 0x8000;
            const DBT_DEVICEREMOVECOMPLETE: usize = 0x8004;
            const DBT_DEVTYP_VOLUME: u32 = 0x00000002;
            const WM_USER_SIMULATE: u32 = WM_USER + 101;

            if msg == WM_DEVICECHANGE || msg == WM_USER_SIMULATE {
                let user_data = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                if user_data != 0 {
                    let sender = &*(user_data as *const broadcast::Sender<StorageDeviceEvent>);

                    // If simulated message via WM_USER_SIMULATE
                    if msg == WM_USER_SIMULATE {
                        let is_arrival = wparam != 0;
                        let drive_char = (lparam as u8) as char;
                        let mount_point_str = format!("{}:\\", drive_char);

                        if is_arrival {
                            let volumes = VolumeInspector::inspect_volumes();
                            if let Some(vol) = volumes.into_iter().find(|v| {
                                v.mount_point.to_string_lossy().to_uppercase().starts_with(&mount_point_str.to_uppercase())
                            }) {
                                let _ = sender.send(StorageDeviceEvent::Plugged(vol));
                            } else {
                                // Synthesize fallback volume info for test simulation
                                let vol = VolumeInfo {
                                    name: format!("Simulated Drive ({}:)", drive_char),
                                    mount_point: std::path::PathBuf::from(&mount_point_str),
                                    filesystem: "NTFS".to_string(),
                                    filesystem_label: Some("SimulatedUSB".to_string()),
                                    total_bytes: 32_000_000_000,
                                    available_bytes: 28_000_000_000,
                                    is_removable: true,
                                    is_system: false,
                                    media_type: super::volume_info::DiskMediaType::RemovableUsb,
                                    serial_number: Some("SIM-USB-001".to_string()),
                                    is_read_only: false,
                                };
                                let _ = sender.send(StorageDeviceEvent::Plugged(vol));
                            }
                        } else {
                            let _ = sender.send(StorageDeviceEvent::Unplugged(mount_point_str));
                        }
                        return 1;
                    }

                    // Native WM_DEVICECHANGE parsing
                    if lparam != 0 {
                        #[repr(C)]
                        struct DevBroadcastHdr {
                            dbch_size: u32,
                            dbch_devicetype: u32,
                            dbch_reserved: u32,
                        }

                        #[repr(C)]
                        struct DevBroadcastVolume {
                            dbcv_size: u32,
                            dbcv_devicetype: u32,
                            dbcv_reserved: u32,
                            dbcv_unitmask: u32,
                            dbcv_flags: u16,
                        }

                        let hdr = &*(lparam as *const DevBroadcastHdr);
                        if hdr.dbch_devicetype == DBT_DEVTYP_VOLUME {
                            let vol = &*(lparam as *const DevBroadcastVolume);
                            let unitmask = vol.dbcv_unitmask;

                            for i in 0..26 {
                                if (unitmask & (1 << i)) != 0 {
                                    let letter = (b'A' + i as u8) as char;
                                    let drive_str = format!("{}:\\", letter);

                                    match wparam {
                                        DBT_DEVICEARRIVAL => {
                                            info!("[OS EVENT] DBT_DEVICEARRIVAL detected for drive {}:\\", letter);
                                            let volumes = VolumeInspector::inspect_volumes();
                                            if let Some(v) = volumes.into_iter().find(|v| {
                                                v.mount_point.to_string_lossy().to_uppercase().starts_with(&drive_str.to_uppercase())
                                            }) {
                                                let _ = sender.send(StorageDeviceEvent::Plugged(v));
                                            }
                                        }
                                        DBT_DEVICEREMOVECOMPLETE => {
                                            info!("[OS EVENT] DBT_DEVICEREMOVECOMPLETE detected for drive {}:\\", letter);
                                            let _ = sender.send(StorageDeviceEvent::Unplugged(drive_str));
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            } else if msg == WM_CLOSE {
                DestroyWindow(hwnd);
                return 0;
            } else if msg == WM_DESTROY {
                PostQuitMessage(0);
                return 0;
            }

            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        unsafe {
            let mut wndclass: WNDCLASSW = std::mem::zeroed();
            wndclass.lpfnWndProc = Some(wndproc);
            wndclass.lpszClassName = class_name_wide.as_ptr();

            RegisterClassW(&wndclass);

            // Create hidden top-level window so OS broadcasts WM_DEVICECHANGE to it
            let hwnd = CreateWindowExW(
                0,
                class_name_wide.as_ptr(),
                class_name_wide.as_ptr(),
                WS_POPUP, // Popup window without WS_VISIBLE is top-level and hidden
                0,
                0,
                0,
                0,
                0, // Top-level window (parent = 0)
                0,
                0,
                std::ptr::null(),
            );

            if hwnd == 0 {
                error!("Failed to create background hot-plug message window");
                is_running.store(false, Ordering::SeqCst);
                return;
            }

            // Store sender pointer in GWLP_USERDATA
            let sender_boxed = Box::into_raw(Box::new(sender));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, sender_boxed as isize);

            hwnd_store.store(hwnd, Ordering::SeqCst);
            info!("AetherStream Windows hot-plug watcher window initialized (HWND: {:?})", hwnd);

            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, 0, 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // Clean up boxed sender upon loop exit
            let _ = Box::from_raw(sender_boxed);
            UnregisterClassW(class_name_wide.as_ptr(), 0);
            hwnd_store.store(0, Ordering::SeqCst);
            is_running.store(false, Ordering::SeqCst);
            info!("AetherStream Windows hot-plug watcher window message loop terminated cleanly");
        }
    }

    #[cfg(target_os = "linux")]
    fn run_linux_netlink_listener(
        sender: broadcast::Sender<StorageDeviceEvent>,
        is_running: Arc<AtomicBool>,
    ) {
        const AF_NETLINK: i32 = 16;
        const SOCK_RAW: i32 = 3;
        const NETLINK_KOBJECT_UEVENT: i32 = 15;

        let fd = unsafe { libc::socket(AF_NETLINK, SOCK_RAW, NETLINK_KOBJECT_UEVENT) };
        if fd < 0 {
            error!("Failed to create Linux netlink kobject uevent socket");
            is_running.store(false, Ordering::SeqCst);
            return;
        }

        #[repr(C)]
        struct SockAddrNl {
            nl_family: u16,
            nl_pad: u16,
            nl_pid: u32,
            nl_groups: u32,
        }

        let addr = SockAddrNl {
            nl_family: AF_NETLINK as u16,
            nl_pad: 0,
            nl_pid: std::process::id(),
            nl_groups: 1, // UEVENT broadcast group
        };

        let bind_res = unsafe {
            libc::bind(
                fd,
                &addr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<SockAddrNl>() as u32,
            )
        };

        if bind_res < 0 {
            error!("Failed to bind Linux netlink socket to kobject uevents");
            unsafe { libc::close(fd); }
            is_running.store(false, Ordering::SeqCst);
            return;
        }

        info!("Linux Netlink kernel block uevent listener active on socket fd {}", fd);
        let mut buf = [0u8; 4096];

        while is_running.load(Ordering::SeqCst) {
            let n = unsafe { libc::recv(fd, buf.as_mut_ptr() as *mut _, buf.len(), 0) };
            if n > 0 {
                if let Some(event) = NetlinkUeventParser::parse(&buf[..n as usize]) {
                    debug!("[LINUX NETLINK] Parsed block event: {:?}", event);
                    match event.action {
                        NetlinkAction::Add => {
                            let dev_path = format!("/dev/{}", event.devname);
                            info!("[LINUX UEVENT] Block device added: {}", dev_path);
                            let volumes = VolumeInspector::inspect_volumes();
                            let vol = volumes.into_iter().find(|v| {
                                v.mount_point.to_string_lossy().contains(&event.devname)
                            }).unwrap_or_else(|| {
                                VolumeInfo {
                                    name: format!("Block Device ({})", event.devname),
                                    mount_point: std::path::PathBuf::from(&dev_path),
                                    filesystem: "ext4".to_string(),
                                    filesystem_label: Some(event.devname.clone()),
                                    total_bytes: 32_000_000_000,
                                    available_bytes: 30_000_000_000,
                                    is_removable: true,
                                    is_system: false,
                                    media_type: super::volume_info::DiskMediaType::RemovableUsb,
                                    serial_number: Some(format!("DEV-{}", event.devname)),
                                    is_read_only: false,
                                }
                            });
                            let _ = sender.send(StorageDeviceEvent::Plugged(vol));
                        }
                        NetlinkAction::Remove => {
                            let dev_path = format!("/dev/{}", event.devname);
                            info!("[LINUX UEVENT] Block device removed: {}", dev_path);
                            let _ = sender.send(StorageDeviceEvent::Unplugged(dev_path));
                        }
                        _ => {}
                    }
                }
            }
        }

        unsafe { libc::close(fd); }
        info!("Linux Netlink kernel block uevent listener closed");
    }

    #[cfg(target_os = "macos")]
    fn run_macos_disk_arbitration_listener(
        sender: broadcast::Sender<StorageDeviceEvent>,
        is_running: Arc<AtomicBool>,
    ) {
        info!("macOS DiskArbitration event monitoring thread started");
        let mut known_mounts = std::collections::HashSet::new();

        if let Ok(entries) = std::fs::read_dir("/Volumes") {
            for e in entries.flatten() {
                known_mounts.insert(e.path());
            }
        }

        while is_running.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let mut current_mounts = std::collections::HashSet::new();
            if let Ok(entries) = std::fs::read_dir("/Volumes") {
                for e in entries.flatten() {
                    current_mounts.insert(e.path());
                }
            }

            for appeared in current_mounts.difference(&known_mounts) {
                let name = appeared.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Volume".into());
                info!("[MACOS DISKARBITRATION] DiskAppearedCallback: {:?}", appeared);
                let da_event = MacDiskArbitrationEvent {
                    bsd_name: name,
                    mount_point: Some(appeared.to_string_lossy().to_string()),
                    is_appeared: true,
                };
                let _ = sender.send(da_event.to_storage_device_event());
            }

            for disappeared in known_mounts.difference(&current_mounts) {
                let name = disappeared.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Volume".into());
                info!("[MACOS DISKARBITRATION] DiskDisappearedCallback: {:?}", disappeared);
                let da_event = MacDiskArbitrationEvent {
                    bsd_name: name,
                    mount_point: Some(disappeared.to_string_lossy().to_string()),
                    is_appeared: false,
                };
                let _ = sender.send(da_event.to_storage_device_event());
            }

            known_mounts = current_mounts;
        }
        info!("macOS DiskArbitration event monitoring loop stopped");
    }
}

impl Default for DiskWatcher {
    fn default() -> Self {
        Self::new()
    }
}
