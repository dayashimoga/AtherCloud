use crate::transcode::profiles::{RenditionProfile, STANDARD_RENDITION_LADDER};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HardwareAccel {
    Nvenc,
    QuickSync,
    Vaapi,
    VideoToolbox,
    SoftwareCpu,
}

impl HardwareAccel {
    pub fn encoder_name(&self) -> &'static str {
        match self {
            HardwareAccel::Nvenc => "h264_nvenc",
            HardwareAccel::QuickSync => "h264_qsv",
            HardwareAccel::Vaapi => "h264_vaapi",
            HardwareAccel::VideoToolbox => "h264_videotoolbox",
            HardwareAccel::SoftwareCpu => "libx264",
        }
    }
}

pub struct TranscodePipeline;

impl TranscodePipeline {
    /// Probes the local system environment for available FFmpeg binary and hardware acceleration support
    pub async fn probe_hardware() -> HardwareAccel {
        let ffmpeg_check = Command::new("ffmpeg")
            .arg("-encoders")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        let output = match ffmpeg_check {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
            _ => {
                warn!("FFmpeg not found on host PATH. Defaulting to Software CPU profile.");
                return HardwareAccel::SoftwareCpu;
            }
        };

        if output.contains("h264_nvenc") {
            info!("NVIDIA NVENC hardware acceleration detected.");
            HardwareAccel::Nvenc
        } else if output.contains("h264_qsv") {
            info!("Intel QuickSync Video (QSV) hardware acceleration detected.");
            HardwareAccel::QuickSync
        } else if output.contains("h264_videotoolbox") {
            info!("Apple VideoToolbox hardware acceleration detected.");
            HardwareAccel::VideoToolbox
        } else if output.contains("h264_vaapi") {
            info!("Linux VA-API hardware acceleration detected.");
            HardwareAccel::Vaapi
        } else {
            info!("No dedicated hardware encoder detected; utilizing libx264 software fallback.");
            HardwareAccel::SoftwareCpu
        }
    }

    /// Generates an RFC 8216 compliant HLS Master Manifest string linking all rendition profiles
    pub fn generate_master_manifest(base_uri_prefix: &str) -> String {
        let mut manifest = String::from("#EXTM3U\n#EXT-X-VERSION:6\n\n");
        for profile in STANDARD_RENDITION_LADDER {
            let line = format!(
                "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={}x{},CODECS=\"{}\"\n{}/{}/index.m3u8\n",
                profile.bandwidth(),
                profile.width,
                profile.height,
                profile.codecs_string,
                base_uri_prefix.trim_end_matches('/'),
                profile.tag
            );
            manifest.push_str(&line);
        }
        manifest
    }

    /// Builds FFmpeg command arguments for JIT HLS multi-rendition segmentation
    pub fn build_ffmpeg_args(
        input_file: &Path,
        output_dir: &Path,
        accel: HardwareAccel,
        profile: &RenditionProfile,
    ) -> Vec<String> {
        let segment_pattern = output_dir.join(format!("{}_%05d.ts", profile.tag));
        let playlist_path = output_dir.join("index.m3u8");

        let mut args = vec![
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "warning".to_string(),
            "-y".to_string(),
            "-i".to_string(),
            input_file.to_string_lossy().to_string(),
        ];

        // Video codec & scale filter
        args.extend(vec![
            "-c:v".to_string(),
            accel.encoder_name().to_string(),
            "-vf".to_string(),
            format!("scale={}:{}", profile.width, profile.height),
            "-b:v".to_string(),
            format!("{}k", profile.video_bitrate_kbps),
            "-maxrate".to_string(),
            format!("{}k", profile.video_bitrate_kbps + 500),
            "-bufsize".to_string(),
            format!("{}k", profile.video_bitrate_kbps * 2),
            "-preset".to_string(),
            "veryfast".to_string(),
        ]);

        // Audio codec
        args.extend(vec![
            "-c:a".to_string(),
            "aac".to_string(),
            "-b:a".to_string(),
            format!("{}k", profile.audio_bitrate_kbps),
            "-ac".to_string(),
            "2".to_string(),
        ]);

        // HLS packaging arguments
        args.extend(vec![
            "-f".to_string(),
            "hls".to_string(),
            "-hls_time".to_string(),
            profile.segment_duration_secs.to_string(),
            "-hls_list_size".to_string(),
            "10".to_string(),
            "-hls_flags".to_string(),
            "delete_segments+independent_segments".to_string(),
            "-hls_segment_filename".to_string(),
            segment_pattern.to_string_lossy().to_string(),
            playlist_path.to_string_lossy().to_string(),
        ]);

        args
    }

    /// Background task to clean orphaned HLS segments older than TTL seconds
    pub async fn spawn_cleanup_worker(scratch_dir: PathBuf, ttl_seconds: u64) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                if !scratch_dir.exists() {
                    continue;
                }

                if let Ok(mut entries) = tokio::fs::read_dir(&scratch_dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("ts") {
                            if let Ok(meta) = entry.metadata().await {
                                if let Ok(modified) = meta.modified() {
                                    if let Ok(elapsed) = modified.elapsed() {
                                        if elapsed.as_secs() > ttl_seconds {
                                            let _ = tokio::fs::remove_file(&path).await;
                                            debug!("Cleaned expired HLS segment: {:?}", path);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    }
}
