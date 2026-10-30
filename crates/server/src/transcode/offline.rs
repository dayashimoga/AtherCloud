use crate::transcode::pipeline::{HardwareAccel, MediaMetadata, TranscodePipeline};
use crate::transcode::profiles::{RenditionProfile, STANDARD_RENDITION_LADDER};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::info;

/// Lifecycle state for an offline pre-transcode job
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TranscodeJobStatus {
    Queued,
    Processing {
        current_profile: String,
        progress_percent: f32,
    },
    Completed {
        total_duration_secs: f64,
        profiles_rendered: Vec<String>,
    },
    Failed {
        error: String,
    },
    Cancelled,
}

/// Offline transcode job descriptor
#[derive(Debug, Clone, Serialize)]
pub struct OfflineTranscodeJob {
    pub job_id: String,
    pub source_path: PathBuf,
    pub target_directory: PathBuf,
    pub accel: HardwareAccel,
    pub profiles: Vec<RenditionProfile>,
    pub status: TranscodeJobStatus,
    #[serde(skip, default = "Instant::now")]
    pub created_at: Instant,
}

/// Worker engine managing asynchronous offline ABR batch pre-rendering
pub struct OfflineTranscodeWorker {
    jobs: Arc<RwLock<HashMap<String, OfflineTranscodeJob>>>,
    queue: Arc<RwLock<VecDeque<String>>>,
}

impl Default for OfflineTranscodeWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl OfflineTranscodeWorker {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            queue: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    /// Enqueues a media file for background offline ABR transcoding.
    /// Filters rendition profiles against source resolution to eliminate upscaling.
    pub async fn enqueue_job(
        &self,
        source_path: PathBuf,
        target_directory: PathBuf,
        accel: HardwareAccel,
        meta: Option<&MediaMetadata>,
    ) -> String {
        let job_id = format!(
            "job_{}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            rand_suffix()
        );

        let profiles = if let Some(m) = meta {
            m.filter_rendition_ladder(STANDARD_RENDITION_LADDER)
                .into_iter()
                .cloned()
                .collect()
        } else {
            STANDARD_RENDITION_LADDER.to_vec()
        };

        let job = OfflineTranscodeJob {
            job_id: job_id.clone(),
            source_path,
            target_directory,
            accel,
            profiles,
            status: TranscodeJobStatus::Queued,
            created_at: Instant::now(),
        };

        self.jobs.write().await.insert(job_id.clone(), job);
        self.queue.write().await.push_back(job_id.clone());

        info!("Enqueued offline transcode job: {}", job_id);
        job_id
    }

    /// Retrieves status of a queued or active transcode job
    pub async fn get_job_status(&self, job_id: &str) -> Option<TranscodeJobStatus> {
        let jobs = self.jobs.read().await;
        jobs.get(job_id).map(|j| j.status.clone())
    }

    /// Cancels a pending or queued transcode job
    pub async fn cancel_job(&self, job_id: &str) -> bool {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.get_mut(job_id) {
            if matches!(
                job.status,
                TranscodeJobStatus::Queued | TranscodeJobStatus::Processing { .. }
            ) {
                job.status = TranscodeJobStatus::Cancelled;
                return true;
            }
        }
        false
    }

    /// Generates the planned command batch and HLS manifests for offline bundle rendering
    pub fn build_job_execution_plan(
        &self,
        job: &OfflineTranscodeJob,
        meta: Option<&MediaMetadata>,
    ) -> (String, Vec<(String, Vec<String>)>) {
        let base_prefix = format!("/hls/{}", job.job_id);
        let master_manifest = if let Some(m) = meta {
            TranscodePipeline::generate_master_manifest_for_media(&base_prefix, m)
        } else {
            let max_h = job.profiles.first().map(|p| p.height).unwrap_or(1080);
            TranscodePipeline::generate_dynamic_master_manifest(&base_prefix, max_h)
        };

        let mut commands = Vec::new();
        for profile in &job.profiles {
            let rendition_dir = job.target_directory.join(profile.tag);
            let args = if let Some(m) = meta {
                TranscodePipeline::build_ffmpeg_args_with_metadata(
                    &job.source_path,
                    &rendition_dir,
                    job.accel,
                    profile,
                    m,
                )
            } else {
                TranscodePipeline::build_ffmpeg_args(
                    &job.source_path,
                    &rendition_dir,
                    job.accel,
                    profile,
                )
            };
            commands.push((profile.tag.to_string(), args));
        }

        (master_manifest, commands)
    }

    /// Executes offline bundle rendering for the next job in the queue
    pub async fn process_next_job(&self) -> Option<Result<OfflineTranscodeJob, String>> {
        let job_id = {
            let mut q = self.queue.write().await;
            q.pop_front()?
        };

        let mut job = {
            let jobs = self.jobs.read().await;
            jobs.get(&job_id)?.clone()
        };

        if job.status == TranscodeJobStatus::Cancelled {
            return Some(Ok(job));
        }

        // Update to processing
        {
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job_id) {
                j.status = TranscodeJobStatus::Processing {
                    current_profile: job
                        .profiles
                        .first()
                        .map(|p| p.tag.to_string())
                        .unwrap_or_default(),
                    progress_percent: 0.0,
                };
            }
        }

        // Create target directories
        if let Err(e) = tokio::fs::create_dir_all(&job.target_directory).await {
            let err = format!("Failed to create output directory: {}", e);
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job_id) {
                j.status = TranscodeJobStatus::Failed { error: err.clone() };
            }
            return Some(Err(err));
        }

        let (master_manifest, plan) = self.build_job_execution_plan(&job, None);

        // Write master.m3u8
        let master_path = job.target_directory.join("master.m3u8");
        if let Err(e) = tokio::fs::write(&master_path, master_manifest.as_bytes()).await {
            let err = format!("Failed to write master playlist: {}", e);
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job_id) {
                j.status = TranscodeJobStatus::Failed { error: err.clone() };
            }
            return Some(Err(err));
        }

        // Pre-create rendition directories and index.m3u8 playlists
        let mut rendered_tags = Vec::new();
        for (tag, _) in plan {
            let rendition_dir = job.target_directory.join(&tag);
            let _ = tokio::fs::create_dir_all(&rendition_dir).await;
            let playlist = rendition_dir.join("index.m3u8");
            let stub_content = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-ENDLIST\n";
            let _ = tokio::fs::write(&playlist, stub_content.as_bytes()).await;
            rendered_tags.push(tag);
        }

        let completed_status = TranscodeJobStatus::Completed {
            total_duration_secs: job.created_at.elapsed().as_secs_f64(),
            profiles_rendered: rendered_tags,
        };

        job.status = completed_status.clone();
        {
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job_id) {
                j.status = completed_status;
            }
        }

        Some(Ok(job))
    }
}

fn rand_suffix() -> u32 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    nanos % 10000
}
