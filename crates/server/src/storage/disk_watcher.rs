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

        #[cfg(not(windows))]
        {
            info!("DiskWatcher initialized (stub listener on non-windows platform)");
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
}

impl Default for DiskWatcher {
    fn default() -> Self {
        Self::new()
    }
}

