use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenditionProfile {
    pub tag: &'static str,
    pub width: u32,
    pub height: u32,
    pub video_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
    pub segment_duration_secs: f32,
    pub max_fps: u32,
    pub codecs_string: &'static str,
}

pub static STANDARD_RENDITION_LADDER: &[RenditionProfile] = &[
    RenditionProfile {
        tag: "1080p_high",
        width: 1920,
        height: 1080,
        video_bitrate_kbps: 4500,
        audio_bitrate_kbps: 192,
        segment_duration_secs: 4.0,
        max_fps: 60,
        codecs_string: "avc1.640028,mp4a.40.2",
    },
    RenditionProfile {
        tag: "720p_med",
        width: 1280,
        height: 720,
        video_bitrate_kbps: 2500,
        audio_bitrate_kbps: 128,
        segment_duration_secs: 4.0,
        max_fps: 60,
        codecs_string: "avc1.4d401f,mp4a.40.2",
    },
    RenditionProfile {
        tag: "480p_low",
        width: 854,
        height: 480,
        video_bitrate_kbps: 1200,
        audio_bitrate_kbps: 96,
        segment_duration_secs: 4.0,
        max_fps: 30,
        codecs_string: "avc1.4d401e,mp4a.40.2",
    },
    RenditionProfile {
        tag: "360p_min",
        width: 640,
        height: 360,
        video_bitrate_kbps: 600,
        audio_bitrate_kbps: 64,
        segment_duration_secs: 4.0,
        max_fps: 30,
        codecs_string: "avc1.42e01e,mp4a.40.2",
    },
];

impl RenditionProfile {
    /// Combined video + audio bandwidth in bits per second (bps) for HLS #EXT-X-STREAM-INF
    pub fn bandwidth(&self) -> u32 {
        (self.video_bitrate_kbps + self.audio_bitrate_kbps) * 1000
    }

    /// Ceiling bitrate with headroom buffer for VBR encoding
    pub fn maxrate_kbps(&self) -> u32 {
        self.video_bitrate_kbps + (self.video_bitrate_kbps / 5).clamp(200, 1000)
    }

    /// Target buffer size for FFmpeg VBR rate control
    pub fn bufsize_kbps(&self) -> u32 {
        self.video_bitrate_kbps * 2
    }

    /// Formats an RFC 8216 compliant HLS #EXT-X-STREAM-INF line
    pub fn to_stream_inf(&self, base_uri_prefix: &str) -> String {
        format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={}x{},FRAME-RATE={:.3},CODECS=\"{}\"\n{}/{}/index.m3u8\n",
            self.bandwidth(),
            self.width,
            self.height,
            self.max_fps as f64,
            self.codecs_string,
            base_uri_prefix.trim_end_matches('/'),
            self.tag
        )
    }
}

/// Lookup a standard rendition profile by tag (e.g. "1080p_high", "720p_med")
pub fn get_profile_by_tag(tag: &str) -> Option<&'static RenditionProfile> {
    STANDARD_RENDITION_LADDER.iter().find(|p| p.tag == tag)
}

/// Selects the optimal ABR rendition profile matching available client bandwidth
/// while bounding resolution to source height (no upscaling).
pub fn select_optimal_profile(
    available_bandwidth_bps: u32,
    source_height: u32,
) -> &'static RenditionProfile {
    let eligible: Vec<&'static RenditionProfile> = STANDARD_RENDITION_LADDER
        .iter()
        .filter(|p| p.height <= source_height)
        .collect();

    if eligible.is_empty() {
        return STANDARD_RENDITION_LADDER.last().unwrap();
    }

    // Allocate 15% safety margin to prevent rebuffering on fluctuating connections
    let effective_budget = (available_bandwidth_bps as f64 * 0.85) as u32;

    for profile in &eligible {
        if profile.bandwidth() <= effective_budget {
            return profile;
        }
    }

    // If budget is lower than all eligible, select the lowest eligible rendition
    eligible.last().unwrap()
}
