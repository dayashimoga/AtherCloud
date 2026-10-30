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

/// Extracted media container, stream codec, and structural properties
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaMetadata {
    pub container: String,
    pub video_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub framerate: Option<f64>,
    pub bitrate_kbps: Option<u64>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<u32>,
    pub gop_size: Option<u32>,
    pub duration_secs: Option<f64>,
}

impl MediaMetadata {
    /// Determines whether the media file can be streamed directly to modern web/mobile clients
    /// without active transcoding (Direct Stream / Direct Play mode).
    pub fn is_direct_streamable(&self) -> bool {
        let container_lower = self.container.to_lowercase();
        let is_valid_container = container_lower.contains("mp4")
            || container_lower.contains("mov")
            || container_lower.contains("webm")
            || container_lower.contains("matroska");

        let is_valid_video = match self.video_codec.as_deref() {
            Some("h264") | Some("avc1") | Some("vp9") | Some("av1") | Some("av01") => true,
            _ => false,
        };

        let is_valid_audio = match self.audio_codec.as_deref() {
            Some("aac") | Some("mp3") | Some("opus") => {
                // Browser direct streaming requires stereo (<= 2 channels); surround sound 5.1/7.1 requires downmixing
                self.audio_channels.unwrap_or(2) <= 2
            }
            None => true, // Video without audio stream is direct streamable
            _ => false,
        };

        is_valid_container && is_valid_video && is_valid_audio
    }

    /// Filters standard adaptive bitrate rendition profiles to prevent wasteful upscaling.
    /// Only profiles with vertical resolution <= source height are retained.
    pub fn filter_rendition_ladder<'a>(
        &self,
        ladder: &'a [RenditionProfile],
    ) -> Vec<&'a RenditionProfile> {
        let max_height = self.height.unwrap_or(1080);
        let mut filtered: Vec<&'a RenditionProfile> = ladder
            .iter()
            .filter(|p| p.height <= max_height)
            .collect();

        if filtered.is_empty() && !ladder.is_empty() {
            if let Some(lowest) = ladder.last() {
                filtered.push(lowest);
            }
        }
        filtered
    }
}

/// JSON deserialization structures for FFprobe raw output
#[derive(Debug, Clone, Deserialize)]
pub struct FFprobeOutput {
    pub streams: Option<Vec<FFprobeStream>>,
    pub format: Option<FFprobeFormat>,
    pub frames: Option<Vec<FFprobeFrame>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FFprobeStream {
    pub index: Option<u32>,
    pub codec_name: Option<String>,
    pub codec_type: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub r_frame_rate: Option<String>,
    pub avg_frame_rate: Option<String>,
    pub bit_rate: Option<String>,
    pub channels: Option<u32>,
    pub channel_layout: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FFprobeFormat {
    pub format_name: Option<String>,
    pub duration: Option<String>,
    pub size: Option<String>,
    pub bit_rate: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FFprobeFrame {
    pub key_frame: Option<i32>,
    pub pict_type: Option<String>,
    pub pts_time: Option<String>,
}

fn parse_framerate(s: &str) -> Option<f64> {
    if let Some((num, den)) = s.split_once('/') {
        let num: f64 = num.parse().ok()?;
        let den: f64 = den.parse().ok()?;
        if den > 0.0 {
            let fps = num / den;
            return Some((fps * 1000.0).round() / 1000.0);
        }
    }
    s.parse::<f64>().ok()
}

pub struct TranscodePipeline;

impl TranscodePipeline {
    /// Parses raw JSON output from FFprobe into strongly typed `MediaMetadata`
    pub fn parse_ffprobe_json(json_str: &str) -> Result<MediaMetadata, String> {
        let output: FFprobeOutput = serde_json::from_str(json_str)
            .map_err(|e| format!("Failed to parse FFprobe JSON output: {}", e))?;

        let mut video_codec = None;
        let mut width = None;
        let mut height = None;
        let mut framerate = None;
        let mut video_bitrate = None;
        let mut audio_codec = None;
        let mut audio_channels = None;

        if let Some(streams) = output.streams {
            for stream in streams {
                let codec_type = stream.codec_type.as_deref().unwrap_or_default();
                if codec_type == "video" && video_codec.is_none() {
                    video_codec = stream.codec_name;
                    width = stream.width;
                    height = stream.height;
                    framerate = stream
                        .avg_frame_rate
                        .as_deref()
                        .and_then(parse_framerate)
                        .or_else(|| stream.r_frame_rate.as_deref().and_then(parse_framerate));
                    if let Some(br_str) = stream.bit_rate {
                        video_bitrate = br_str.parse::<u64>().ok().map(|b| b / 1000);
                    }
                } else if codec_type == "audio" && audio_codec.is_none() {
                    audio_codec = stream.codec_name;
                    audio_channels = stream.channels;
                }
            }
        }

        let mut container = String::new();
        let mut duration_secs = None;
        let mut format_bitrate = None;

        if let Some(fmt) = output.format {
            if let Some(name) = fmt.format_name {
                container = name;
            }
            if let Some(dur) = fmt.duration {
                duration_secs = dur.parse::<f64>().ok();
            }
            if let Some(br) = fmt.bit_rate {
                format_bitrate = br.parse::<u64>().ok().map(|b| b / 1000);
            }
        }

        // Calculate GOP (Group of Pictures) interval by measuring distance between keyframes
        let mut gop_size = None;
        if let Some(frames) = output.frames {
            let mut keyframe_indices = Vec::new();
            for (idx, frame) in frames.iter().enumerate() {
                if frame.key_frame == Some(1) || frame.pict_type.as_deref() == Some("I") {
                    keyframe_indices.push(idx);
                }
            }

            if keyframe_indices.len() >= 2 {
                let diff = keyframe_indices[1] - keyframe_indices[0];
                if diff > 0 {
                    gop_size = Some(diff as u32);
                }
            } else if keyframe_indices.len() == 1 && frames.len() > 1 {
                gop_size = Some(frames.len() as u32);
            }
        }

        let bitrate_kbps = video_bitrate.or(format_bitrate);

        Ok(MediaMetadata {
            container,
            video_codec,
            width,
            height,
            framerate,
            bitrate_kbps,
            audio_codec,
            audio_channels,
            gop_size,
            duration_secs,
        })
    }

    /// Probes media container and stream metadata using the host FFprobe binary
    pub async fn probe_file(file_path: &Path) -> Result<MediaMetadata, String> {
        if !file_path.exists() {
            return Err(format!("Input file does not exist: {:?}", file_path));
        }

        let probe_cmd = Command::new("ffprobe")
            .args(&[
                "-v",
                "quiet",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
                "-show_frames",
                "-select_streams",
                "v:0",
                "-read_intervals",
                "%+#60",
                file_path.to_str().unwrap_or_default(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match probe_cmd {
            Ok(output) if output.status.success() => {
                let json_text = String::from_utf8_lossy(&output.stdout);
                Self::parse_ffprobe_json(&json_text)
            }
            Ok(output) => {
                let err_msg = String::from_utf8_lossy(&output.stderr);
                Err(format!("FFprobe exited with error: {}", err_msg))
            }
            Err(e) => {
                warn!(
                    "FFprobe executable not found on host PATH ({}); utilizing fallback container detection.",
                    e
                );
                let ext = file_path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_lowercase();
                let container = match ext.as_str() {
                    "mp4" | "m4v" => "mov,mp4,m4a,3gp,3g2,mj2".to_string(),
                    "mkv" => "matroska,webm".to_string(),
                    "webm" => "matroska,webm".to_string(),
                    "avi" => "avi".to_string(),
                    _ => ext,
                };
                Ok(MediaMetadata {
                    container,
                    video_codec: Some("h264".to_string()),
                    width: Some(1920),
                    height: Some(1080),
                    framerate: Some(30.0),
                    bitrate_kbps: Some(4000),
                    audio_codec: Some("aac".to_string()),
                    audio_channels: Some(2),
                    gop_size: Some(60),
                    duration_secs: None,
                })
            }
        }
    }

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
