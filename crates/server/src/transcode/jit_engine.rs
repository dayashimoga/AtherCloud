use crate::transcode::pipeline::{
    CachedSegment, GopAlignmentConfig, HardwareAccel, LruSegmentRingBuffer, MediaMetadata,
    TranscodePipeline,
};
use crate::transcode::profiles::{get_profile_by_tag, RenditionProfile};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Operating state of an active on-demand JIT transcoding session
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JitSessionState {
    /// Session created, waiting for first segment request
    Idle,
    /// Actively generating segments towards forward buffer quota
    Active,
    /// Paused because forward buffer reached or exceeded 30s threshold
    Paused,
    /// Media file fully transcoded up to EOF
    Completed,
    /// Session stopped or invalidated due to seek or teardown
    Stopped,
}

impl JitSessionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            JitSessionState::Idle => "idle",
            JitSessionState::Active => "active",
            JitSessionState::Paused => "paused",
            JitSessionState::Completed => "completed",
            JitSessionState::Stopped => "stopped",
        }
    }
}

/// Dynamic buffering configuration for JIT segment generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JitSegmentConfig {
    /// Forward buffer duration threshold in seconds before pausing generation (default: 30.0s)
    pub target_forward_buffer_secs: f32,
    /// Low-water mark in seconds below which paused generation automatically resumes (default: 16.0s)
    pub low_watermark_secs: f32,
    /// Nominal segment duration in seconds (default: 4.0s)
    pub segment_duration_secs: f32,
    /// Maximum idle duration in seconds before session is cleaned up (default: 60s)
    pub idle_timeout_secs: u64,
    /// In-memory ring buffer segment capacity (default: 20 segments)
    pub ring_buffer_capacity: usize,
}

impl Default for JitSegmentConfig {
    fn default() -> Self {
        Self {
            target_forward_buffer_secs: 30.0,
            low_watermark_secs: 16.0,
            segment_duration_secs: 4.0,
            idle_timeout_secs: 60,
            ring_buffer_capacity: 20,
        }
    }
}

/// Real-time diagnostic telemetry snapshot for an active JIT transcode session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JitSessionMetrics {
    pub session_id: String,
    pub stream_key: String,
    pub profile_tag: String,
    pub state: String,
    pub forward_buffer_secs: f32,
    pub client_seq: u64,
    pub head_seq: u64,
    pub cached_segments_count: usize,
    pub total_cached_bytes: usize,
    pub is_paused: bool,
    pub idle_duration_secs: f32,
}

/// An active on-demand JIT transcoding session managing keyframe-aligned segment caching
/// and 30-second forward buffer flow control.
pub struct JitTranscodeSession {
    pub session_id: String,
    pub stream_key: String,
    pub source_path: PathBuf,
    pub output_dir: PathBuf,
    pub profile: RenditionProfile,
    pub meta: MediaMetadata,
    pub accel: HardwareAccel,
    pub config: JitSegmentConfig,
    pub gop_config: GopAlignmentConfig,
    state: Arc<RwLock<JitSessionState>>,
    current_client_seq: Arc<AtomicU64>,
    transcoded_head_seq: Arc<AtomicU64>,
    ring_buffer: Arc<RwLock<LruSegmentRingBuffer>>,
    last_active_at: Arc<RwLock<Instant>>,
    created_at: Instant,
}

impl JitTranscodeSession {
    /// Instantiates a new JIT transcode session for a specific rendition
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: String,
        stream_key: String,
        source_path: PathBuf,
        output_dir: PathBuf,
        profile: RenditionProfile,
        meta: MediaMetadata,
        accel: HardwareAccel,
        config: JitSegmentConfig,
    ) -> Self {
        let gop_config = GopAlignmentConfig::for_profile_and_metadata(&profile, Some(&meta));
        let ring_buffer = LruSegmentRingBuffer::new(config.ring_buffer_capacity);

        Self {
            session_id,
            stream_key,
            source_path,
            output_dir,
            profile,
            meta,
            accel,
            config,
            gop_config,
            state: Arc::new(RwLock::new(JitSessionState::Idle)),
            current_client_seq: Arc::new(AtomicU64::new(0)),
            transcoded_head_seq: Arc::new(AtomicU64::new(0)),
            ring_buffer: Arc::new(RwLock::new(ring_buffer)),
            last_active_at: Arc::new(RwLock::new(Instant::now())),
            created_at: Instant::now(),
        }
    }

    /// Computes the current depth of the forward buffer ahead of client playback in seconds
    pub fn forward_buffer_depth_secs(&self) -> f32 {
        let head = self.transcoded_head_seq.load(Ordering::SeqCst);
        let client = self.current_client_seq.load(Ordering::SeqCst);
        if head > client {
            (head - client) as f32 * self.config.segment_duration_secs
        } else {
            0.0
        }
    }

    /// Returns true if the forward buffer has reached or exceeded the target pause threshold (30s)
    pub fn should_pause_worker(&self) -> bool {
        self.forward_buffer_depth_secs() >= self.config.target_forward_buffer_secs
    }

    /// Returns true if the forward buffer has drained below the low-water mark (16s)
    pub fn should_resume_worker(&self) -> bool {
        self.forward_buffer_depth_secs() < self.config.low_watermark_secs
    }

    /// Current session state
    pub async fn state(&self) -> JitSessionState {
        self.state.read().await.clone()
    }

    /// Checks if session generation is currently paused
    pub async fn is_paused(&self) -> bool {
        matches!(*self.state.read().await, JitSessionState::Paused)
    }

    /// Transitions session to Paused
    pub async fn pause(&self) {
        let mut st = self.state.write().await;
        if *st == JitSessionState::Active || *st == JitSessionState::Idle {
            *st = JitSessionState::Paused;
            debug!(
                "JIT session {} paused: buffer depth {:.1}s >= {:.1}s",
                self.session_id,
                self.forward_buffer_depth_secs(),
                self.config.target_forward_buffer_secs
            );
        }
    }

    /// Transitions session to Active
    pub async fn resume(&self) {
        let mut st = self.state.write().await;
        if *st == JitSessionState::Paused || *st == JitSessionState::Idle {
            *st = JitSessionState::Active;
            debug!(
                "JIT session {} resumed: buffer depth {:.1}s < {:.1}s",
                self.session_id,
                self.forward_buffer_depth_secs(),
                self.config.low_watermark_secs
            );
        }
    }

    /// Terminates the session
    pub async fn stop(&self) {
        let mut st = self.state.write().await;
        *st = JitSessionState::Stopped;
    }

    /// Formats the segment filename for a given sequence number
    pub fn segment_name_for_seq(&self, seq: u64) -> String {
        format!("{}_{:05}.ts", self.profile.tag, seq)
    }

    /// Extracts sequence number from a segment filename
    pub fn parse_seq_from_segment_name(&self, name: &str) -> Option<u64> {
        let prefix = format!("{}_", self.profile.tag);
        let trimmed = name.strip_prefix(&prefix)?;
        let num_str = trimmed.strip_suffix(".ts")?;
        num_str.parse::<u64>().ok()
    }

    /// Client request entry point: touches recency, updates client position, evaluates
    /// forward buffer state transitions (resume if drained), and retrieves segment from cache.
    pub async fn request_segment(&self, seq: u64) -> Option<CachedSegment> {
        *self.last_active_at.write().await = Instant::now();

        // Update client playback position monotonically
        self.current_client_seq.fetch_max(seq, Ordering::SeqCst);

        // Flow control: If paused and buffer has drained below low-water mark, resume generation
        let buffer_depth = self.forward_buffer_depth_secs();
        let mut st = self.state.write().await;
        if *st == JitSessionState::Paused && buffer_depth < self.config.low_watermark_secs {
            *st = JitSessionState::Active;
            info!(
                "JIT transcode flow control resumed for session {}: buffer drained to {:.1}s",
                self.session_id, buffer_depth
            );
        } else if *st == JitSessionState::Idle {
            *st = JitSessionState::Active;
        }
        drop(st);

        // Fetch segment from in-memory ring buffer
        let segment_name = self.segment_name_for_seq(seq);
        let mut ring = self.ring_buffer.write().await;
        ring.get_segment(&segment_name).cloned()
    }

    /// Commits a newly generated transport segment into the ring buffer, advances
    /// the transcoded head counter, and triggers a pause if the forward buffer target (30s) is reached.
    pub async fn commit_segment(
        &self,
        seq: u64,
        data: Option<Bytes>,
        size_bytes: usize,
    ) -> Option<String> {
        let segment_name = self.segment_name_for_seq(seq);
        let mut ring = self.ring_buffer.write().await;
        let evicted = ring.insert_segment(segment_name, seq, data, size_bytes);
        drop(ring);

        // Advance head sequence counter to at least seq + 1
        self.transcoded_head_seq.fetch_max(seq + 1, Ordering::SeqCst);

        // Flow control: If forward buffer >= 30s, enter Paused state to eliminate CPU/GPU waste
        if self.should_pause_worker() {
            let mut st = self.state.write().await;
            if *st == JitSessionState::Active {
                *st = JitSessionState::Paused;
                info!(
                    "JIT transcode flow control paused for session {}: buffer reached {:.1}s (threshold: {:.1}s)",
                    self.session_id,
                    self.forward_buffer_depth_secs(),
                    self.config.target_forward_buffer_secs
                );
            }
        }

        evicted
    }

    /// Checks if this session has been inactive longer than the configured idle timeout
    pub async fn is_idle_expired(&self) -> bool {
        self.last_active_at.read().await.elapsed()
            > Duration::from_secs(self.config.idle_timeout_secs)
    }

    /// Collects real-time diagnostic metrics
    pub async fn metrics(&self) -> JitSessionMetrics {
        let state = self.state.read().await;
        let ring = self.ring_buffer.read().await;
        let idle_secs = self.last_active_at.read().await.elapsed().as_secs_f32();

        JitSessionMetrics {
            session_id: self.session_id.clone(),
            stream_key: self.stream_key.clone(),
            profile_tag: self.profile.tag.to_string(),
            state: state.as_str().to_string(),
            forward_buffer_secs: self.forward_buffer_depth_secs(),
            client_seq: self.current_client_seq.load(Ordering::SeqCst),
            head_seq: self.transcoded_head_seq.load(Ordering::SeqCst),
            cached_segments_count: ring.len(),
            total_cached_bytes: ring.total_cached_bytes(),
            is_paused: matches!(*state, JitSessionState::Paused),
            idle_duration_secs: idle_secs,
        }
    }

    /// Age of this session since creation
    pub fn uptime(&self) -> Duration {
        self.created_at.elapsed()
    }
}

/// Central registry managing lifecycle, memory ring buffers, and on-demand JIT transcode sessions
pub struct JitTranscodeManager {
    sessions: Arc<RwLock<HashMap<String, Arc<JitTranscodeSession>>>>,
    scratch_dir: PathBuf,
    config: JitSegmentConfig,
}

impl JitTranscodeManager {
    /// Creates a new JIT transcode manager
    pub fn new(scratch_dir: PathBuf, config: JitSegmentConfig) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            scratch_dir,
            config,
        }
    }

    /// Computes a unique session key for a mount, relative media path, and rendition profile tag
    pub fn session_key(mount: &str, subpath: &Path, profile_tag: &str) -> String {
        format!("{}:{}:{}", mount, subpath.to_string_lossy(), profile_tag)
    }

    /// Retrieves an existing session or creates and registers a new JIT session
    pub async fn get_or_create_session(
        &self,
        mount: &str,
        subpath: &Path,
        profile_tag: &str,
        source_path: PathBuf,
        meta: MediaMetadata,
        accel: HardwareAccel,
    ) -> Result<Arc<JitTranscodeSession>, String> {
        let key = Self::session_key(mount, subpath, profile_tag);

        let read_guard = self.sessions.read().await;
        if let Some(session) = read_guard.get(&key) {
            return Ok(session.clone());
        }
        drop(read_guard);

        let profile = get_profile_by_tag(profile_tag)
            .ok_or_else(|| format!("Unknown rendition profile tag: {}", profile_tag))?;

        let session_output_dir = self.scratch_dir.join("hls_jit").join(key.replace(':', "_"));
        let session = Arc::new(JitTranscodeSession::new(
            key.clone(),
            key.clone(),
            source_path,
            session_output_dir,
            profile.clone(),
            meta,
            accel,
            self.config.clone(),
        ));

        let mut write_guard = self.sessions.write().await;
        write_guard.insert(key, session.clone());
        Ok(session)
    }

    /// Retrieves an active session by its lookup key
    pub async fn get_session(&self, session_key: &str) -> Option<Arc<JitTranscodeSession>> {
        self.sessions.read().await.get(session_key).cloned()
    }

    /// Reaps expired or abandoned idle transcode sessions
    pub async fn reap_idle_sessions(&self) -> usize {
        let mut guard = self.sessions.write().await;
        let mut expired_keys = Vec::new();

        for (k, session) in guard.iter() {
            if session.is_idle_expired().await {
                expired_keys.push(k.clone());
            }
        }

        let reaped_count = expired_keys.len();
        for k in expired_keys {
            if let Some(session) = guard.remove(&k) {
                session.stop().await;
                debug!("Reaped idle JIT transcode session: {}", k);
            }
        }

        reaped_count
    }

    /// Total count of active sessions in memory
    pub async fn active_session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Diagnostic list of all active sessions and their buffer health
    pub async fn list_active_sessions(&self) -> Vec<JitSessionMetrics> {
        let guard = self.sessions.read().await;
        let mut list = Vec::with_capacity(guard.len());
        for session in guard.values() {
            list.push(session.metrics().await);
        }
        list
    }

    /// Generates an RFC 8216 media rendition playlist (`index.m3u8`) for the session,
    /// serving ready segments with correct sequence indices and 4.0s duration tags.
    pub async fn generate_rendition_playlist(
        &self,
        session: &JitTranscodeSession,
        base_url_prefix: &str,
    ) -> String {
        let head_seq = session.transcoded_head_seq.load(Ordering::SeqCst);
        let client_seq = session.current_client_seq.load(Ordering::SeqCst);
        let ring = session.ring_buffer.read().await;

        let mut playlist = String::with_capacity(1024);
        playlist.push_str("#EXTM3U\n");
        playlist.push_str("#EXT-X-VERSION:6\n");
        playlist.push_str(&format!(
            "#EXT-X-TARGETDURATION:{:.0}\n",
            session.config.segment_duration_secs
        ));
        playlist.push_str(&format!(
            "#EXT-X-MEDIA-SEQUENCE:{}\n",
            client_seq.min(head_seq)
        ));
        playlist.push_str("#EXT-X-PLAYLIST-TYPE:EVENT\n\n");

        let start_seq = client_seq.saturating_sub(2);
        let end_seq = head_seq;

        for seq in start_seq..end_seq {
            let seg_name = session.segment_name_for_seq(seq);
            if ring.has_segment(&seg_name) || seq < head_seq {
                playlist.push_str(&format!(
                    "#EXTINF:{:.6},\n{}/{}\n",
                    session.config.segment_duration_secs, base_url_prefix, seg_name
                ));
            }
        }

        playlist
    }
}
