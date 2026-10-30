use crate::transcode::profiles::{RenditionProfile, STANDARD_RENDITION_LADDER};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tracing::{debug, info, warn};

/// In-memory cached transport segment descriptor
#[derive(Debug, Clone)]
pub struct CachedSegment {
    pub segment_name: String,
    pub sequence: u64,
    pub size_bytes: usize,
    pub created_at: Instant,
    pub data: Option<bytes::Bytes>,
}

/// Rolling LRU ring buffer managing JIT segment cache and proactive disk eviction
#[derive(Debug)]
pub struct LruSegmentRingBuffer {
    max_segments: usize,
    segments: HashMap<String, CachedSegment>,
    order: VecDeque<String>,
}

impl LruSegmentRingBuffer {
    pub fn new(max_segments: usize) -> Self {
        Self {
            max_segments: max_segments.max(1),
            segments: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Inserts a newly generated transport segment into the ring buffer.
    /// If capacity is exceeded, evicts the oldest segment key for storage cleanup.
    pub fn insert_segment(
        &mut self,
        name: String,
        sequence: u64,
        data: Option<bytes::Bytes>,
        size_bytes: usize,
    ) -> Option<String> {
        let mut evicted = None;

        if self.segments.contains_key(&name) {
            self.order.retain(|k| k != &name);
        } else if self.segments.len() >= self.max_segments {
            if let Some(oldest_key) = self.order.pop_front() {
                self.segments.remove(&oldest_key);
                evicted = Some(oldest_key);
            }
        }

        self.order.push_back(name.clone());
        self.segments.insert(
            name.clone(),
            CachedSegment {
                segment_name: name,
                sequence,
                size_bytes,
                created_at: Instant::now(),
                data,
            },
        );

        evicted
    }

    /// Retrieves a segment from cache and touches its recency position
    pub fn get_segment(&mut self, name: &str) -> Option<&CachedSegment> {
        if self.segments.contains_key(name) {
            let key = name.to_string();
            self.order.retain(|k| k != &key);
            self.order.push_back(key);
            self.segments.get(name)
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn total_cached_bytes(&self) -> usize {
        self.segments.values().map(|s| s.size_bytes).sum()
    }

    pub fn clear(&mut self) {
        self.segments.clear();
        self.order.clear();
    }
}

/// Strategy for processing media audio during adaptive bitrate streaming
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioTranscodeStrategy {
    /// Audio is already stereo AAC; copy directly with zero CPU transcoding overhead
    PassThrough,
    /// Multi-channel surround sound (5.1 or 7.1) downmixed to 2-channel stereo AAC
    DownmixSurroundToStereo,
    /// Non-AAC stereo codec (e.g. MP3, Opus, FLAC, AC-3) transcoded to standard AAC
    TranscodeToStereoAac,
    /// Source media has no audio track
    NoAudio,
}

impl AudioTranscodeStrategy {
    /// Determines the optimal audio processing strategy given media metadata
    pub fn determine(meta: &MediaMetadata) -> Self {
        let codec = match meta.audio_codec.as_deref() {
            Some(c) => c.to_lowercase(),
            None => return AudioTranscodeStrategy::NoAudio,
        };
        let channels = meta.audio_channels.unwrap_or(2);

        if channels > 2 {
            AudioTranscodeStrategy::DownmixSurroundToStereo
        } else if codec == "aac" || codec == "mp4a" {
            AudioTranscodeStrategy::PassThrough
        } else {
            AudioTranscodeStrategy::TranscodeToStereoAac
        }
    }

    /// Emits FFmpeg CLI switches implementing this audio strategy
    pub fn ffmpeg_flags(&self, target_bitrate_kbps: u32) -> Vec<String> {
        match self {
            AudioTranscodeStrategy::PassThrough => vec![
                "-c:a".to_string(),
                "copy".to_string(),
            ],
            AudioTranscodeStrategy::DownmixSurroundToStereo => vec![
                "-c:a".to_string(),
                "aac".to_string(),
                "-b:a".to_string(),
                format!("{}k", target_bitrate_kbps.min(192)),
                "-ac".to_string(),
                "2".to_string(),
                "-af".to_string(),
                "aresample=matrix_encoding=dplii".to_string(),
            ],
            AudioTranscodeStrategy::TranscodeToStereoAac => vec![
                "-c:a".to_string(),
                "aac".to_string(),
                "-b:a".to_string(),
                format!("{}k", target_bitrate_kbps),
                "-ac".to_string(),
                "2".to_string(),
            ],
            AudioTranscodeStrategy::NoAudio => vec![
                "-an".to_string(),
            ],
        }
    }
}

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

    pub fn hevc_encoder_name(&self) -> &'static str {
        match self {
            HardwareAccel::Nvenc => "hevc_nvenc",
            HardwareAccel::QuickSync => "hevc_qsv",
            HardwareAccel::Vaapi => "hevc_vaapi",
            HardwareAccel::VideoToolbox => "hevc_videotoolbox",
            HardwareAccel::SoftwareCpu => "libx265",
        }
    }

    pub fn is_hardware_accelerated(&self) -> bool {
        !matches!(self, HardwareAccel::SoftwareCpu)
    }

    pub fn description(&self) -> &'static str {
        match self {
            HardwareAccel::Nvenc => "NVIDIA NVENC (CUDA Accelerated)",
            HardwareAccel::QuickSync => "Intel QuickSync Video (QSV)",
            HardwareAccel::Vaapi => "Linux Video Acceleration API (VA-API)",
            HardwareAccel::VideoToolbox => "Apple VideoToolbox (CoreVideo / Metal)",
            HardwareAccel::SoftwareCpu => "Software CPU (libx264 veryfast fallback)",
        }
    }

    pub fn extra_ffmpeg_flags(&self) -> Vec<&'static str> {
        match self {
            HardwareAccel::Nvenc => vec!["-hwaccel", "cuda"],
            HardwareAccel::QuickSync => vec!["-hwaccel", "qsv"],
            HardwareAccel::Vaapi => vec!["-hwaccel", "vaapi"],
            HardwareAccel::VideoToolbox => vec!["-hwaccel", "videotoolbox"],
            HardwareAccel::SoftwareCpu => vec![],
        }
    }

    /// Evaluates the priority ladder across available FFmpeg encoders.
    /// Order of preference: NVENC -> QuickSync -> VideoToolbox -> VA-API -> libx264.
    pub fn detect_from_encoder_list(output: &str) -> Self {
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

        HardwareAccel::detect_from_encoder_list(&output)
    }

    /// Generates an RFC 8216 compliant HLS Master Manifest string linking all rendition profiles
    pub fn generate_master_manifest(base_uri_prefix: &str) -> String {
        Self::generate_dynamic_master_manifest(base_uri_prefix, 1080)
    }

    /// Generates an RFC 8216 compliant dynamic HLS Master Manifest string
    /// bounded by the source video height to strictly prevent upscaling.
    pub fn generate_dynamic_master_manifest(base_uri_prefix: &str, source_height: u32) -> String {
        let mut manifest = String::from("#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-INDEPENDENT-SEGMENTS\n\n");
        let mut profiles: Vec<&RenditionProfile> = STANDARD_RENDITION_LADDER
            .iter()
            .filter(|p| p.height <= source_height)
            .collect();

        if profiles.is_empty() && !STANDARD_RENDITION_LADDER.is_empty() {
            if let Some(lowest) = STANDARD_RENDITION_LADDER.last() {
                profiles.push(lowest);
            }
        }

        for profile in profiles {
            manifest.push_str(&profile.to_stream_inf(base_uri_prefix));
        }
        manifest
    }

    /// Generates an RFC 8216 compliant dynamic HLS Master Manifest directly from extracted MediaMetadata
    pub fn generate_master_manifest_for_media(
        base_uri_prefix: &str,
        meta: &MediaMetadata,
    ) -> String {
        let source_height = meta.height.unwrap_or(1080);
        Self::generate_dynamic_master_manifest(base_uri_prefix, source_height)
    }

    /// Builds FFmpeg command arguments for JIT HLS multi-rendition segmentation
    pub fn build_ffmpeg_args(
        input_file: &Path,
        output_dir: &Path,
        accel: HardwareAccel,
        profile: &RenditionProfile,
    ) -> Vec<String> {
        Self::build_ffmpeg_args_with_strategy(
            input_file,
            output_dir,
            accel,
            profile,
            AudioTranscodeStrategy::TranscodeToStereoAac,
        )
    }

    /// Builds FFmpeg command arguments using source MediaMetadata to automatically determine
    /// whether audio pass-through or dynamic 5.1/7.1 stereo downmixing should be engaged.
    pub fn build_ffmpeg_args_with_metadata(
        input_file: &Path,
        output_dir: &Path,
        accel: HardwareAccel,
        profile: &RenditionProfile,
        meta: &MediaMetadata,
    ) -> Vec<String> {
        let strategy = AudioTranscodeStrategy::determine(meta);
        Self::build_ffmpeg_args_with_strategy(input_file, output_dir, accel, profile, strategy)
    }

    /// Builds FFmpeg command arguments with an explicit AudioTranscodeStrategy
    pub fn build_ffmpeg_args_with_strategy(
        input_file: &Path,
        output_dir: &Path,
        accel: HardwareAccel,
        profile: &RenditionProfile,
        audio_strategy: AudioTranscodeStrategy,
    ) -> Vec<String> {
        let segment_pattern = output_dir.join(format!("{}_%05d.ts", profile.tag));
        let playlist_path = output_dir.join("index.m3u8");

        let mut args = vec![
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "warning".to_string(),
            "-y".to_string(),
        ];

        for flag in accel.extra_ffmpeg_flags() {
            args.push(flag.to_string());
        }

        args.extend(vec![
            "-i".to_string(),
            input_file.to_string_lossy().to_string(),
        ]);

        let gop_size = ((profile.max_fps as f32) * profile.segment_duration_secs).round() as u32;

        // Video codec & scale filter
        args.extend(vec![
            "-c:v".to_string(),
            accel.encoder_name().to_string(),
            "-vf".to_string(),
            format!("scale={}:{}", profile.width, profile.height),
            "-b:v".to_string(),
            format!("{}k", profile.video_bitrate_kbps),
            "-maxrate".to_string(),
            format!("{}k", profile.maxrate_kbps()),
            "-bufsize".to_string(),
            format!("{}k", profile.bufsize_kbps()),
            "-g".to_string(),
            gop_size.to_string(),
            "-keyint_min".to_string(),
            gop_size.to_string(),
            "-sc_threshold".to_string(),
            "0".to_string(),
            "-preset".to_string(),
            "veryfast".to_string(),
        ]);

        // Audio flags driven by dynamic transcode strategy
        args.extend(audio_strategy.ffmpeg_flags(profile.audio_bitrate_kbps));

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
