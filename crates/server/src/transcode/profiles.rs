use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenditionProfile {
    pub tag: &'static str,
    pub width: u32,
    pub height: u32,
    pub video_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
    pub segment_duration_secs: f32,
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
        codecs_string: "avc1.640028,mp4a.40.2",
    },
    RenditionProfile {
        tag: "720p_med",
        width: 1280,
        height: 720,
        video_bitrate_kbps: 2500,
        audio_bitrate_kbps: 128,
        segment_duration_secs: 4.0,
        codecs_string: "avc1.4d401f,mp4a.40.2",
    },
    RenditionProfile {
        tag: "480p_low",
        width: 854,
        height: 480,
        video_bitrate_kbps: 1200,
        audio_bitrate_kbps: 96,
        segment_duration_secs: 4.0,
        codecs_string: "avc1.4d401e,mp4a.40.2",
    },
    RenditionProfile {
        tag: "360p_min",
        width: 640,
        height: 360,
        video_bitrate_kbps: 600,
        audio_bitrate_kbps: 64,
        segment_duration_secs: 4.0,
        codecs_string: "avc1.42e01e,mp4a.40.2",
    },
];

impl RenditionProfile {
    pub fn bandwidth(&self) -> u32 {
        (self.video_bitrate_kbps + self.audio_bitrate_kbps) * 1000
    }
}
