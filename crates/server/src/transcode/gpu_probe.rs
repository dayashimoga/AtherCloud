use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{info, warn};

use super::pipeline::HardwareAccel;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceleratorCapabilities {
    pub accel: HardwareAccel,
    pub available: bool,
    pub h264_encoder: Option<String>,
    pub hevc_encoder: Option<String>,
    pub av1_encoder: Option<String>,
    pub hwaccel_framework: Option<String>,
    pub driver_info: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GpuCapabilityReport {
    pub primary_accel: HardwareAccel,
    pub nvenc: AcceleratorCapabilities,
    pub qsv: AcceleratorCapabilities,
    pub videotoolbox: AcceleratorCapabilities,
    pub vaapi: AcceleratorCapabilities,
    pub cpu_fallback: AcceleratorCapabilities,
    pub supported_hwaccels: Vec<String>,
    pub supported_encoders: Vec<String>,
    pub os_platform: String,
    pub dri_device_accessible: bool,
    pub detection_logs: Vec<String>,
}

impl GpuCapabilityReport {
    /// Returns true if a hardware-accelerated encoder was successfully detected
    pub fn is_hardware_accelerated(&self) -> bool {
        self.primary_accel.is_hardware_accelerated()
    }

    /// Returns the active encoder string for the given accelerator and video codec
    pub fn get_encoder_for(&self, accel: HardwareAccel, codec: &str) -> Option<&str> {
        let caps = match accel {
            HardwareAccel::Nvenc => &self.nvenc,
            HardwareAccel::QuickSync => &self.qsv,
            HardwareAccel::VideoToolbox => &self.videotoolbox,
            HardwareAccel::Vaapi => &self.vaapi,
            HardwareAccel::SoftwareCpu => &self.cpu_fallback,
        };
        match codec.to_lowercase().as_str() {
            "h264" | "avc" => caps.h264_encoder.as_deref(),
            "h265" | "hevc" => caps.hevc_encoder.as_deref(),
            "av1" => caps.av1_encoder.as_deref(),
            _ => None,
        }
    }
}

pub struct GpuProbeEngine;

impl GpuProbeEngine {
    /// Standard enterprise priority ladder:
    /// NVENC (NVIDIA) -> QuickSync (Intel) -> VideoToolbox (Apple) -> VA-API (Linux) -> Software CPU (libx264)
    pub fn priority_ladder() -> &'static [HardwareAccel] {
        &[
            HardwareAccel::Nvenc,
            HardwareAccel::QuickSync,
            HardwareAccel::VideoToolbox,
            HardwareAccel::Vaapi,
            HardwareAccel::SoftwareCpu,
        ]
    }

    /// Evaluates dynamic GPU and hardware acceleration capabilities from raw FFmpeg `-hwaccels` and `-encoders` outputs
    pub fn parse_capabilities(hwaccels_output: &str, encoders_output: &str) -> GpuCapabilityReport {
        let mut supported_hwaccels = Vec::new();
        for line in hwaccels_output.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with("Hardware acceleration methods") {
                supported_hwaccels.push(trimmed.to_lowercase());
            }
        }

        let mut supported_encoders = Vec::new();
        for line in encoders_output.lines() {
            let trimmed = line.trim();
            // Format: V..... encoder_name description
            if trimmed.starts_with('V') {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    supported_encoders.push(parts[1].to_lowercase());
                }
            } else if line.contains("h264_") || line.contains("hevc_") || line.contains("libx264") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                for p in parts {
                    if p.starts_with("h264_") || p.starts_with("hevc_") || p.starts_with("av1_") || p.starts_with("libx") {
                        supported_encoders.push(p.to_lowercase());
                    }
                }
            }
        }

        // Deduplicate
        supported_hwaccels.sort();
        supported_hwaccels.dedup();
        supported_encoders.sort();
        supported_encoders.dedup();

        let os_platform = if cfg!(windows) {
            "windows".to_string()
        } else if cfg!(target_os = "macos") {
            "macos".to_string()
        } else {
            "linux".to_string()
        };

        let dri_device_accessible = Path::new("/dev/dri/renderD128").exists()
            || Path::new("/dev/dri/card0").exists();

        let mut logs = Vec::new();

        // 1. NVIDIA NVENC (CUDA)
        let nvenc_h264 = supported_encoders.iter().any(|e| e == "h264_nvenc");
        let nvenc_hevc = supported_encoders.iter().any(|e| e == "hevc_nvenc");
        let nvenc_av1 = supported_encoders.iter().any(|e| e == "av1_nvenc");
        let nvenc_avail = nvenc_h264;
        let nvenc_caps = AcceleratorCapabilities {
            accel: HardwareAccel::Nvenc,
            available: nvenc_avail,
            h264_encoder: if nvenc_h264 { Some("h264_nvenc".to_string()) } else { None },
            hevc_encoder: if nvenc_hevc { Some("hevc_nvenc".to_string()) } else { None },
            av1_encoder: if nvenc_av1 { Some("av1_nvenc".to_string()) } else { None },
            hwaccel_framework: Some("cuda".to_string()),
            driver_info: Some("NVIDIA NVENC / CUDA".to_string()),
        };
        logs.push(format!(
            "NVENC: available={}, h264={}, hevc={}, av1={}",
            nvenc_avail, nvenc_h264, nvenc_hevc, nvenc_av1
        ));

        // 2. Intel QuickSync Video (QSV)
        let qsv_h264 = supported_encoders.iter().any(|e| e == "h264_qsv");
        let qsv_hevc = supported_encoders.iter().any(|e| e == "hevc_qsv");
        let qsv_av1 = supported_encoders.iter().any(|e| e == "av1_qsv");
        let qsv_avail = qsv_h264;
        let qsv_caps = AcceleratorCapabilities {
            accel: HardwareAccel::QuickSync,
            available: qsv_avail,
            h264_encoder: if qsv_h264 { Some("h264_qsv".to_string()) } else { None },
            hevc_encoder: if qsv_hevc { Some("hevc_qsv".to_string()) } else { None },
            av1_encoder: if qsv_av1 { Some("av1_qsv".to_string()) } else { None },
            hwaccel_framework: Some("qsv".to_string()),
            driver_info: Some("Intel QuickSync Video (oneVPL / MSDK)".to_string()),
        };
        logs.push(format!(
            "QuickSync: available={}, h264={}, hevc={}, av1={}",
            qsv_avail, qsv_h264, qsv_hevc, qsv_av1
        ));

        // 3. Apple VideoToolbox
        let vt_h264 = supported_encoders.iter().any(|e| e == "h264_videotoolbox");
        let vt_hevc = supported_encoders.iter().any(|e| e == "hevc_videotoolbox");
        let vt_avail = vt_h264 && (cfg!(target_os = "macos") || !supported_encoders.is_empty());
        let vt_caps = AcceleratorCapabilities {
            accel: HardwareAccel::VideoToolbox,
            available: vt_avail,
            h264_encoder: if vt_h264 { Some("h264_videotoolbox".to_string()) } else { None },
            hevc_encoder: if vt_hevc { Some("hevc_videotoolbox".to_string()) } else { None },
            av1_encoder: None,
            hwaccel_framework: Some("videotoolbox".to_string()),
            driver_info: Some("Apple VideoToolbox (Metal / CoreVideo)".to_string()),
        };
        logs.push(format!(
            "VideoToolbox: available={}, h264={}, hevc={}",
            vt_avail, vt_h264, vt_hevc
        ));

        // 4. Linux VA-API
        let vaapi_h264 = supported_encoders.iter().any(|e| e == "h264_vaapi");
        let vaapi_hevc = supported_encoders.iter().any(|e| e == "hevc_vaapi");
        let vaapi_av1 = supported_encoders.iter().any(|e| e == "av1_vaapi");
        let vaapi_avail = vaapi_h264;
        let vaapi_caps = AcceleratorCapabilities {
            accel: HardwareAccel::Vaapi,
            available: vaapi_avail,
            h264_encoder: if vaapi_h264 { Some("h264_vaapi".to_string()) } else { None },
            hevc_encoder: if vaapi_hevc { Some("hevc_vaapi".to_string()) } else { None },
            av1_encoder: if vaapi_av1 { Some("av1_vaapi".to_string()) } else { None },
            hwaccel_framework: Some("vaapi".to_string()),
            driver_info: Some("Linux VA-API (libva / DRM)".to_string()),
        };
        logs.push(format!(
            "VA-API: available={}, h264={}, hevc={}, dri_device={}",
            vaapi_avail, vaapi_h264, vaapi_hevc, dri_device_accessible
        ));

        // 5. Software CPU Fallback (libx264 / libx265)
        let cpu_h264 = supported_encoders.iter().any(|e| e == "libx264") || supported_encoders.is_empty();
        let cpu_hevc = supported_encoders.iter().any(|e| e == "libx265");
        let cpu_av1 = supported_encoders.iter().any(|e| e == "libsvtav1");
        let cpu_caps = AcceleratorCapabilities {
            accel: HardwareAccel::SoftwareCpu,
            available: true,
            h264_encoder: Some("libx264".to_string()),
            hevc_encoder: if cpu_hevc { Some("libx265".to_string()) } else { None },
            av1_encoder: if cpu_av1 { Some("libsvtav1".to_string()) } else { None },
            hwaccel_framework: None,
            driver_info: Some("Software CPU (libx264 veryfast)".to_string()),
        };
        logs.push(format!(
            "CPU Fallback: available=true, h264={}, hevc={}, av1={}",
            cpu_h264, cpu_hevc, cpu_av1
        ));

        // Ladder Selection: NVENC -> QuickSync -> VideoToolbox -> VA-API -> Software CPU
        let primary_accel = if nvenc_avail {
            HardwareAccel::Nvenc
        } else if qsv_avail {
            HardwareAccel::QuickSync
        } else if vt_avail {
            HardwareAccel::VideoToolbox
        } else if vaapi_avail {
            HardwareAccel::Vaapi
        } else {
            HardwareAccel::SoftwareCpu
        };

        logs.push(format!("Primary Accelerator Selected: {:?}", primary_accel));

        GpuCapabilityReport {
            primary_accel,
            nvenc: nvenc_caps,
            qsv: qsv_caps,
            videotoolbox: vt_caps,
            vaapi: vaapi_caps,
            cpu_fallback: cpu_caps,
            supported_hwaccels,
            supported_encoders,
            os_platform,
            dri_device_accessible,
            detection_logs: logs,
        }
    }

    /// Asynchronously probes the host system at runtime by executing `ffmpeg -hwaccels` and `ffmpeg -encoders`
    pub async fn probe_system() -> GpuCapabilityReport {
        let hwaccels_output = match Command::new("ffmpeg")
            .arg("-hwaccels")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
        {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
            _ => String::new(),
        };

        let encoders_output = match Command::new("ffmpeg")
            .arg("-encoders")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
        {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
            _ => {
                warn!("FFmpeg binary not detected on PATH; defaulting to Software CPU profile.");
                String::new()
            }
        };

        let report = Self::parse_capabilities(&hwaccels_output, &encoders_output);
        info!(
            "GPU Probe Complete: Primary Accelerator = {:?} ({})",
            report.primary_accel,
            report.primary_accel.description()
        );
        report
    }
}

