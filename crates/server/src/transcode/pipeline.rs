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

    /// Checks if a segment exists in the ring buffer without altering its LRU position
    pub fn has_segment(&self, name: &str) -> bool {
        self.segments.contains_key(name)
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
        let is_surround = if let Some(ch) = meta.audio_channels {
            ch > 2
        } else {
            meta.audio_layout.as_deref().is_some_and(|layout| {
                let lower = layout.to_lowercase();
                lower.contains("5.1")
                    || lower.contains("7.1")
                    || lower.contains("6.1")
                    || lower.contains("surround")
            })
        };

        if is_surround {
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

    /// Returns the target channel count for downstream web player compatibility
    pub fn target_channels(&self) -> u32 {
        match self {
            AudioTranscodeStrategy::PassThrough => 2,
            AudioTranscodeStrategy::DownmixSurroundToStereo => 2,
            AudioTranscodeStrategy::TranscodeToStereoAac => 2,
            AudioTranscodeStrategy::NoAudio => 0,
        }
    }

    /// Returns whether active audio transcoding/downmixing is performed (false for direct pass-through or silent)
    pub fn is_transcoding_required(&self) -> bool {
        matches!(
            self,
            AudioTranscodeStrategy::DownmixSurroundToStereo
                | AudioTranscodeStrategy::TranscodeToStereoAac
        )
    }

    /// Human-readable description of audio processing mode
    pub fn description(&self) -> &'static str {
        match self {
            AudioTranscodeStrategy::PassThrough => "Direct copy (Stereo AAC pass-through)",
            AudioTranscodeStrategy::DownmixSurroundToStereo => {
                "Surround sound (5.1/7.1) downmixed to Stereo AAC with Pro Logic II matrix encoding"
            }
            AudioTranscodeStrategy::TranscodeToStereoAac => "Stereo audio transcoded to AAC",
            AudioTranscodeStrategy::NoAudio => "No audio stream",
        }
    }
}

/// Configuration for exact 4-second GOP (Group of Pictures) keyframe alignment and HLS segmenting.
/// Guarantees that every segment boundary coincides with an IDR keyframe and sequence parameter set (SPS/PPS),
/// eliminating video stutter, dropped frames, and audio-video desync during playback and bitrate transitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GopAlignmentConfig {
    /// Target segment duration in seconds (standard: 4.0s)
    pub segment_duration_secs: f32,
    /// Effective framerate in frames-per-second (e.g. 23.976, 24.0, 25.0, 29.97, 30.0, 50.0, 59.94, 60.0)
    pub effective_fps: f32,
    /// Calculated exact GOP size in frames: round(effective_fps * segment_duration_secs)
    pub gop_size: u32,
    /// Minimum GOP keyframe distance (-keyint_min), locked equal to gop_size to prevent variable-interval keyframes
    pub keyint_min: u32,
    /// Scene cut threshold (-sc_threshold), locked to 0 to prevent scene changes from shifting keyframes off grid
    pub sc_threshold: u32,
    /// FFmpeg forced keyframes expression: "expr:gte(t,n_forced*{duration})"
    pub force_key_frames_expr: String,
    /// HLS playlist flags ensuring independent segment decoding and cleanup
    pub hls_flags: String,
    /// Rolling HLS playlist size (number of segment entries preserved in index.m3u8)
    pub hls_list_size: u32,
}

impl Default for GopAlignmentConfig {
    fn default() -> Self {
        Self::new(30.0, 4.0)
    }
}

impl GopAlignmentConfig {
    /// Creates a new GOP alignment configuration with exact keyframe spacing
    pub fn new(fps: f32, segment_duration_secs: f32) -> Self {
        let fps_clamped = if fps <= 0.0 { 30.0 } else { fps };
        let duration = if segment_duration_secs <= 0.0 { 4.0 } else { segment_duration_secs };
        let gop_size = (fps_clamped * duration).round() as u32;
        let force_key_frames_expr = format!("expr:gte(t,n_forced*{:.0})", duration);
        let hls_flags = "delete_segments+independent_segments".to_string();

        Self {
            segment_duration_secs: duration,
            effective_fps: fps_clamped,
            gop_size,
            keyint_min: gop_size,
            sc_threshold: 0,
            force_key_frames_expr,
            hls_flags,
            hls_list_size: 10,
        }
    }

    /// Derives GOP alignment from a target rendition profile and source media metadata
    pub fn for_profile_and_metadata(
        profile: &RenditionProfile,
        meta: Option<&MediaMetadata>,
    ) -> Self {
        let target_duration = if profile.segment_duration_secs > 0.0 {
            profile.segment_duration_secs
        } else {
            4.0
        };

        let effective_fps = match meta.and_then(|m| m.framerate) {
            Some(fps) if fps > 0.0 => (fps as f32).min(profile.max_fps as f32),
            _ => profile.max_fps as f32,
        };

        Self::new(effective_fps, target_duration)
    }

    /// Computes the duration of a single video frame in milliseconds
    pub fn frame_duration_ms(&self) -> f32 {
        1000.0 / self.effective_fps
    }

    /// Computes the exact nominal segment duration in milliseconds
    pub fn segment_duration_ms(&self) -> f32 {
        self.segment_duration_secs * 1000.0
    }

    /// Computes the expected frame count in an exact-aligned segment
    pub fn expected_frames_per_segment(&self) -> u32 {
        self.gop_size
    }

    /// Validates whether an observed keyframe timestamp (in seconds) falls on an expected GOP boundary
    pub fn is_exact_boundary(&self, timestamp_secs: f32, tolerance_secs: f32) -> bool {
        if timestamp_secs < 0.0 {
            return false;
        }
        let remainder = timestamp_secs % self.segment_duration_secs;
        remainder <= tolerance_secs || (self.segment_duration_secs - remainder) <= tolerance_secs
    }

    /// Validates an array of PTS timestamps (e.g. from ffprobe frames) to ensure continuous keyframe alignment
    pub fn validate_keyframe_sequence(&self, keyframe_pts: &[f32], tolerance_ms: f32) -> bool {
        if keyframe_pts.is_empty() {
            return false;
        }
        let tolerance_secs = tolerance_ms / 1000.0;
        for &pts in keyframe_pts {
            if !self.is_exact_boundary(pts, tolerance_secs) {
                return false;
            }
        }
        true
    }

    /// Returns FFmpeg CLI options configuring encoder GOP and keyframe pacing
    pub fn ffmpeg_gop_flags(&self) -> Vec<String> {
        vec![
            "-g".to_string(),
            self.gop_size.to_string(),
            "-keyint_min".to_string(),
            self.keyint_min.to_string(),
            "-sc_threshold".to_string(),
            self.sc_threshold.to_string(),
            "-force_key_frames".to_string(),
            self.force_key_frames_expr.clone(),
        ]
    }

    /// Returns FFmpeg CLI options configuring the HLS segmenter
    pub fn ffmpeg_hls_flags(&self, segment_pattern: &Path, playlist_path: &Path) -> Vec<String> {
        vec![
            "-f".to_string(),
            "hls".to_string(),
            "-hls_time".to_string(),
            format!("{:.0}", self.segment_duration_secs),
            "-hls_list_size".to_string(),
            self.hls_list_size.to_string(),
            "-hls_flags".to_string(),
            self.hls_flags.clone(),
            "-hls_segment_filename".to_string(),
            segment_pattern.to_string_lossy().to_string(),
            playlist_path.to_string_lossy().to_string(),
        ]
    }

    /// Human-readable diagnostic summary of GOP alignment
    pub fn summary(&self) -> String {
        format!(
            "GOP Size: {} frames | Keyint Min: {} | SC Threshold: {} | Segment: {:.1}s | Effective FPS: {:.3} | Forced Expr: {}",
            self.gop_size, self.keyint_min, self.sc_threshold, self.segment_duration_secs, self.effective_fps, self.force_key_frames_expr
        )
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
        super::gpu_probe::GpuProbeEngine::parse_capabilities("", output).primary_accel
    }
}

/// Extracted media container, stream codec, and structural properties
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaMetadata {
    pub container: String,
    pub video_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub aspect_ratio: Option<String>,
    pub framerate: Option<f64>,
    pub bitrate_kbps: Option<u64>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<u32>,
    pub audio_layout: Option<String>,
    pub audio_sample_rate: Option<u32>,
    pub pix_fmt: Option<String>,
    pub gop_size: Option<u32>,
    pub duration_secs: Option<f64>,
}

impl MediaMetadata {
    /// Computes display aspect ratio string from pixel dimensions
    pub fn calculate_aspect_ratio(width: u32, height: u32) -> String {
        if width == 0 || height == 0 {
            return "unknown".to_string();
        }
        fn gcd(mut a: u32, mut b: u32) -> u32 {
            while b != 0 {
                let t = b;
                b = a % b;
                a = t;
            }
            a
        }
        let div = gcd(width, height);
        let rw = width / div;
        let rh = height / div;
        if rw <= 64 && rh <= 64 {
            format!("{}:{}", rw, rh)
        } else {
            let ratio = (width as f64) / (height as f64);
            format!("{:.2}:1", ratio)
        }
    }

    /// Infers human-readable audio layout name from channel count
    pub fn infer_audio_layout(channels: u32) -> &'static str {
        match channels {
            1 => "mono",
            2 => "stereo",
            3 => "2.1",
            4 => "4.0",
            5 => "5.0",
            6 => "5.1",
            7 => "6.1",
            8 => "7.1",
            _ => "surround",
        }
    }

    /// Determines whether the media file can be streamed directly to modern web/mobile clients
    /// without active transcoding (Direct Stream / Direct Play mode).
    pub fn is_direct_streamable(&self) -> bool {
        let container_lower = self.container.to_lowercase();
        let is_valid_container = container_lower.contains("mp4")
            || container_lower.contains("mov")
            || container_lower.contains("webm")
            || container_lower.contains("matroska");

        let is_valid_video = matches!(
            self.video_codec.as_deref(),
            Some("h264") | Some("avc1") | Some("vp9") | Some("av1") | Some("av01")
        );

        let is_valid_audio = match self.audio_codec.as_deref() {
            Some("aac") | Some("mp3") | Some("opus") => {
                let is_surround = if let Some(ch) = self.audio_channels {
                    ch > 2
                } else {
                    self.audio_layout.as_deref().is_some_and(|layout| {
                        let l = layout.to_lowercase();
                        l.contains("5.1")
                            || l.contains("7.1")
                            || l.contains("6.1")
                            || l.contains("surround")
                    })
                };
                !is_surround
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
    pub codec_long_name: Option<String>,
    pub profile: Option<String>,
    pub codec_type: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub display_aspect_ratio: Option<String>,
    pub sample_aspect_ratio: Option<String>,
    pub pix_fmt: Option<String>,
    pub r_frame_rate: Option<String>,
    pub avg_frame_rate: Option<String>,
    pub bit_rate: Option<String>,
    pub channels: Option<u32>,
    pub channel_layout: Option<String>,
    pub sample_rate: Option<String>,
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
        let mut aspect_ratio = None;
        let mut framerate = None;
        let mut video_bitrate = None;
        let mut pix_fmt = None;
        let mut audio_codec = None;
        let mut audio_channels = None;
        let mut audio_layout = None;
        let mut audio_sample_rate = None;

        if let Some(streams) = output.streams {
            for stream in streams {
                let codec_type = stream.codec_type.as_deref().unwrap_or_default();
                if codec_type == "video" && video_codec.is_none() {
                    video_codec = stream.codec_name;
                    width = stream.width;
                    height = stream.height;
                    pix_fmt = stream.pix_fmt;

                    let dar_clean = stream.display_aspect_ratio.as_deref().filter(|dar| {
                        !dar.is_empty() && *dar != "0:1" && *dar != "N/A"
                    });

                    aspect_ratio = dar_clean
                        .map(|s| s.to_string())
                        .or_else(|| {
                            match (width, height) {
                                (Some(w), Some(h)) if w > 0 && h > 0 => {
                                    Some(MediaMetadata::calculate_aspect_ratio(w, h))
                                }
                                _ => None,
                            }
                        });

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
                    audio_layout = stream
                        .channel_layout
                        .filter(|layout| !layout.is_empty() && layout != "unknown")
                        .or_else(|| {
                            audio_channels
                                .map(MediaMetadata::infer_audio_layout)
                                .map(str::to_string)
                        });
                    audio_sample_rate = stream.sample_rate.and_then(|s| s.parse::<u32>().ok());
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
            aspect_ratio,
            framerate,
            bitrate_kbps,
            audio_codec,
            audio_channels,
            audio_layout,
            audio_sample_rate,
            pix_fmt,
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
            .args([
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
                    aspect_ratio: Some("16:9".to_string()),
                    framerate: Some(30.0),
                    bitrate_kbps: Some(4000),
                    audio_codec: Some("aac".to_string()),
                    audio_channels: Some(2),
                    audio_layout: Some("stereo".to_string()),
                    audio_sample_rate: Some(48000),
                    pix_fmt: Some("yuv420p".to_string()),
                    gop_size: Some(60),
                    duration_secs: None,
                })
            }
        }
    }

    /// Probes the local system environment for available FFmpeg binary and hardware acceleration support
    pub async fn probe_hardware() -> HardwareAccel {
        if let Some(ladder) = super::gpu_probe::get_validated_ladder() {
            return ladder.active_accel;
        }
        let (_report, ladder) = Self::probe_and_validate_hardware().await;
        ladder.active_accel
    }

    /// Performs full dynamic GPU capability introspection across NVENC, QuickSync, VideoToolbox, VA-API, and CPU fallback
    pub async fn probe_capabilities() -> super::gpu_probe::GpuCapabilityReport {
        if let Some(report) = super::gpu_probe::get_capability_report() {
            return report;
        }
        let (report, _ladder) = Self::probe_and_validate_hardware().await;
        report
    }

    /// Performs full dynamic GPU capability probe followed by 1-frame transcode dry-run validation with clean ladder failover
    pub async fn probe_and_validate_hardware() -> (
        super::gpu_probe::GpuCapabilityReport,
        super::gpu_probe::ValidatedEncoderLadder,
    ) {
        super::gpu_probe::GpuProbeEngine::probe_and_validate_system().await
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

    /// Generates an RFC 8216 compliant dynamic HLS Master Manifest directly from extracted MediaMetadata,
    /// adapting rendition resolution to source aspect ratio, bounding framerate, and clamping bitrates.
    pub fn generate_master_manifest_for_media(
        base_uri_prefix: &str,
        meta: &MediaMetadata,
    ) -> String {
        let mut manifest = String::from("#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-INDEPENDENT-SEGMENTS\n\n");

        let eligible_profiles = meta.filter_rendition_ladder(STANDARD_RENDITION_LADDER);

        let aspect_ratio = match (meta.width, meta.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w as f64) / (h as f64),
            _ => 16.0 / 9.0,
        };

        for profile in eligible_profiles {
            let target_height = profile.height;
            let target_width = if let (Some(w), Some(h)) = (meta.width, meta.height) {
                if (w as f64 / h as f64 - 16.0 / 9.0).abs() < 0.02 {
                    profile.width
                } else {
                    let raw_w = (target_height as f64 * aspect_ratio).round() as u32;
                    raw_w & !1 // Ensure even width for h264 macroblock alignment
                }
            } else {
                profile.width
            };

            let effective_fps = meta
                .framerate
                .unwrap_or(profile.max_fps as f64)
                .min(profile.max_fps as f64);

            let effective_video_kbps = if let Some(src_kbps) = meta.bitrate_kbps {
                profile.video_bitrate_kbps.min(src_kbps as u32)
            } else {
                profile.video_bitrate_kbps
            };

            let stream_inf = profile.to_custom_stream_inf(
                base_uri_prefix,
                target_width,
                target_height,
                effective_fps,
                effective_video_kbps,
            );
            manifest.push_str(&stream_inf);
        }

        manifest
    }

    /// Returns the active GopAlignmentConfig for a profile and optional source metadata
    pub fn get_gop_alignment_config(
        profile: &RenditionProfile,
        meta: Option<&MediaMetadata>,
    ) -> GopAlignmentConfig {
        GopAlignmentConfig::for_profile_and_metadata(profile, meta)
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
    /// audio pass-through or dynamic 5.1/7.1 stereo downmixing and exact GOP alignment.
    pub fn build_ffmpeg_args_with_metadata(
        input_file: &Path,
        output_dir: &Path,
        accel: HardwareAccel,
        profile: &RenditionProfile,
        meta: &MediaMetadata,
    ) -> Vec<String> {
        let strategy = AudioTranscodeStrategy::determine(meta);
        Self::build_ffmpeg_args_full(
            input_file,
            output_dir,
            accel,
            profile,
            strategy,
            Some(meta),
        )
    }

    /// Builds FFmpeg command arguments with an explicit AudioTranscodeStrategy
    pub fn build_ffmpeg_args_with_strategy(
        input_file: &Path,
        output_dir: &Path,
        accel: HardwareAccel,
        profile: &RenditionProfile,
        audio_strategy: AudioTranscodeStrategy,
    ) -> Vec<String> {
        Self::build_ffmpeg_args_full(
            input_file,
            output_dir,
            accel,
            profile,
            audio_strategy,
            None,
        )
    }

    /// Builds comprehensive FFmpeg command arguments with exact GOP keyframe alignment,
    /// dynamic audio downmixing/passthrough strategy, and metadata-aware scaling.
    pub fn build_ffmpeg_args_full(
        input_file: &Path,
        output_dir: &Path,
        accel: HardwareAccel,
        profile: &RenditionProfile,
        audio_strategy: AudioTranscodeStrategy,
        meta: Option<&MediaMetadata>,
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

        let gop_config = GopAlignmentConfig::for_profile_and_metadata(profile, meta);

        // Compute aspect-ratio preserved dimensions if source metadata is provided
        let (scaled_width, scaled_height) = if let Some(m) = meta {
            if let (Some(w), Some(h)) = (m.width, m.height) {
                if h > 0 && w > 0 {
                    let target_height = profile.height.min(h);
                    let aspect = w as f64 / h as f64;
                    let target_width = ((target_height as f64 * aspect).round() as u32) & !1;
                    (target_width.max(2), target_height)
                } else {
                    (profile.width, profile.height)
                }
            } else {
                (profile.width, profile.height)
            }
        } else {
            (profile.width, profile.height)
        };

        // Video codec & scale filter
        args.extend(vec![
            "-c:v".to_string(),
            accel.encoder_name().to_string(),
            "-vf".to_string(),
            format!("scale={}:{}", scaled_width, scaled_height),
            "-b:v".to_string(),
            format!("{}k", profile.video_bitrate_kbps),
            "-maxrate".to_string(),
            format!("{}k", profile.maxrate_kbps()),
            "-bufsize".to_string(),
            format!("{}k", profile.bufsize_kbps()),
        ]);

        // Keyframe GOP alignment flags (exact 4-second GOP)
        args.extend(gop_config.ffmpeg_gop_flags());

        args.extend(vec![
            "-preset".to_string(),
            "veryfast".to_string(),
        ]);

        // Audio flags driven by dynamic transcode strategy
        args.extend(audio_strategy.ffmpeg_flags(profile.audio_bitrate_kbps));

        // HLS packaging arguments with exact segment duration
        args.extend(gop_config.ffmpeg_hls_flags(&segment_pattern, &playlist_path));

        args
    }

    /// Background task to clean orphaned HLS segments older than TTL seconds
    pub async fn spawn_cleanup_worker(scratch_dir: PathBuf, ttl_seconds: u64) {
        let config = DiskCacheConfig {
            scratch_dir,
            max_cache_bytes: 500 * 1024 * 1024,
            ttl_seconds,
            target_watermark_ratio: 0.80,
        };
        DiskCacheSupervisor::spawn_supervisor(config, Duration::from_secs(30));
    }
}

/// Configuration parameters for transcode scratch storage quota and TTL enforcement
#[derive(Debug, Clone)]
pub struct DiskCacheConfig {
    pub scratch_dir: PathBuf,
    pub max_cache_bytes: u64,
    pub ttl_seconds: u64,
    pub target_watermark_ratio: f64,
}

impl Default for DiskCacheConfig {
    fn default() -> Self {
        Self {
            scratch_dir: PathBuf::from("scratch/transcode"),
            max_cache_bytes: 500 * 1024 * 1024, // 500 MiB default quota
            ttl_seconds: 60,                     // 60-second inactivity TTL per NFR-303
            target_watermark_ratio: 0.80,        // Reclaim to 80% watermark on breach
        }
    }
}

/// Disk cache supervisor enforcing storage quotas and LRU eviction
pub struct DiskCacheSupervisor;

impl DiskCacheSupervisor {
    /// Executes a single pass of the disk cache quota and TTL eviction algorithm,
    /// returning `(evicted_files_count, reclaimed_bytes)`.
    pub async fn sweep_cache(config: &DiskCacheConfig) -> std::io::Result<(usize, u64)> {
        if !config.scratch_dir.exists() {
            return Ok((0, 0));
        }

        let mut entries = tokio::fs::read_dir(&config.scratch_dir).await?;
        let mut file_records: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
        let mut evicted_count = 0usize;
        let mut reclaimed_bytes = 0u64;

        // Pass 1: Scan and collect file metadata, evicting TTL-expired segments immediately
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if let Ok(meta) = entry.metadata().await {
                if !meta.is_file() {
                    continue;
                }
                let len = meta.len();
                let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);

                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed.as_secs_f64() >= config.ttl_seconds as f64
                        && tokio::fs::remove_file(&path).await.is_ok()
                    {
                        evicted_count += 1;
                        reclaimed_bytes += len;
                        debug!("Evicted TTL-expired segment: {:?}", path);
                        continue;
                    }
                }

                file_records.push((path, len, modified));
            }
        }

        // Pass 2: Calculate remaining disk usage and enforce quota via LRU eviction
        let mut current_total_bytes: u64 = file_records.iter().map(|(_, len, _)| *len).sum();

        if current_total_bytes > config.max_cache_bytes {
            let target_bytes =
                (config.max_cache_bytes as f64 * config.target_watermark_ratio) as u64;

            // Sort files by modification time ascending (oldest first)
            file_records.sort_by_key(|(_, _, modified)| *modified);

            for (path, len, _) in file_records {
                if current_total_bytes <= target_bytes {
                    break;
                }
                if tokio::fs::remove_file(&path).await.is_ok() {
                    evicted_count += 1;
                    reclaimed_bytes += len;
                    current_total_bytes = current_total_bytes.saturating_sub(len);
                    debug!("Evicted LRU segment for quota relief: {:?}", path);
                }
            }
        }

        Ok((evicted_count, reclaimed_bytes))
    }

    /// Spawns a background task running periodic sweeps on the transcode cache directory
    pub fn spawn_supervisor(
        config: DiskCacheConfig,
        interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                if let Err(e) = Self::sweep_cache(&config).await {
                    warn!("Disk cache supervisor sweep encountered error: {:?}", e);
                }
            }
        })
    }
}
