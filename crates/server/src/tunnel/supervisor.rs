use crate::tunnel::binary_manager::{BinaryError, BinaryManagerConfig, CloudflaredBinaryManager};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, RwLock};

#[derive(Debug, thiserror::Error)]
pub enum TunnelSupervisorError {
    #[error("Binary resolution error: {0}")]
    BinaryResolution(#[from] BinaryError),
    #[error("Process spawn error: {0}")]
    ProcessSpawn(#[from] std::io::Error),
    #[error("Tunnel already running with PID {0}")]
    AlreadyRunning(u32),
    #[error("Supervisor task join error: {0}")]
    JoinError(String),
    #[error("Termination failed: {0}")]
    TerminationFailed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelState {
    Stopped,
    Starting,
    Running,
    Restarting,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TunnelEvent {
    Starting {
        target_url: String,
    },
    Running {
        pid: u32,
    },
    UnexpectedExit {
        exit_code: Option<i32>,
        retry_count: u32,
        backoff_secs: u64,
    },
    Restarting {
        retry_count: u32,
    },
    Failed {
        reason: String,
    },
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelSupervisorConfig {
    pub target_url: String,
    pub tunnel_token: Option<String>,
    pub custom_binary_path: Option<PathBuf>,
    pub sidecar_dir: PathBuf,
    pub allow_auto_download: bool,
    pub auto_restart: bool,
    pub max_retries: u32,
    pub initial_backoff_secs: u64,
    pub max_backoff_secs: u64,
    pub stable_uptime_threshold_secs: u64,
    pub extra_args: Vec<String>,
    pub command_override: Option<Vec<String>>,
}

impl Default for TunnelSupervisorConfig {
    fn default() -> Self {
        Self {
            target_url: "http://127.0.0.1:8080".to_string(),
            tunnel_token: None,
            custom_binary_path: None,
            sidecar_dir: PathBuf::from("bin"),
            allow_auto_download: true,
            auto_restart: true,
            max_retries: 5,
            initial_backoff_secs: 1,
            max_backoff_secs: 30,
            stable_uptime_threshold_secs: 60,
            extra_args: Vec::new(),
            command_override: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelSupervisorStatus {
    pub state: TunnelState,
    pub pid: Option<u32>,
    pub uptime_secs: u64,
    pub restart_count: u32,
    pub last_exit_code: Option<i32>,
    pub binary_path: Option<PathBuf>,
    pub target_url: String,
}

struct SupervisorInner {
    config: Arc<RwLock<TunnelSupervisorConfig>>,
    state: Arc<RwLock<TunnelState>>,
    pid: Arc<RwLock<Option<u32>>>,
    started_at: Arc<RwLock<Option<Instant>>>,
    restart_count: Arc<RwLock<u32>>,
    last_exit_code: Arc<RwLock<Option<i32>>>,
    resolved_binary: Arc<RwLock<Option<PathBuf>>>,
    event_tx: broadcast::Sender<TunnelEvent>,
    output_tx: broadcast::Sender<String>,
    shutdown_tx: broadcast::Sender<()>,
    supervisor_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
}

impl Drop for SupervisorInner {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(());
    }
}

#[derive(Clone)]
pub struct CloudflaredSupervisor {
    inner: Arc<SupervisorInner>,
}

impl CloudflaredSupervisor {
    pub fn new(config: TunnelSupervisorConfig) -> Self {
        let (event_tx, _) = broadcast::channel(128);
        let (output_tx, _) = broadcast::channel(512);
        let (shutdown_tx, _) = broadcast::channel(1);

        Self {
            inner: Arc::new(SupervisorInner {
                config: Arc::new(RwLock::new(config)),
                state: Arc::new(RwLock::new(TunnelState::Stopped)),
                pid: Arc::new(RwLock::new(None)),
                started_at: Arc::new(RwLock::new(None)),
                restart_count: Arc::new(RwLock::new(0)),
                last_exit_code: Arc::new(RwLock::new(None)),
                resolved_binary: Arc::new(RwLock::new(None)),
                event_tx,
                output_tx,
                shutdown_tx,
                supervisor_handle: Arc::new(RwLock::new(None)),
            }),
        }
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<TunnelEvent> {
        self.inner.event_tx.subscribe()
    }

    pub fn subscribe_output(&self) -> broadcast::Receiver<String> {
        self.inner.output_tx.subscribe()
    }

    pub async fn state(&self) -> TunnelState {
        *self.inner.state.read().await
    }

    pub async fn is_running(&self) -> bool {
        *self.inner.state.read().await == TunnelState::Running
    }

    pub async fn status(&self) -> TunnelSupervisorStatus {
        let state = *self.inner.state.read().await;
        let pid = *self.inner.pid.read().await;
        let uptime_secs = self
            .inner
            .started_at
            .read()
            .await
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);
        let restart_count = *self.inner.restart_count.read().await;
        let last_exit_code = *self.inner.last_exit_code.read().await;
        let binary_path = self.inner.resolved_binary.read().await.clone();
        let target_url = self.inner.config.read().await.target_url.clone();

        TunnelSupervisorStatus {
            state,
            pid,
            uptime_secs,
            restart_count,
            last_exit_code,
            binary_path,
            target_url,
        }
    }

    pub async fn start(&self) -> Result<(), TunnelSupervisorError> {
        let current_state = *self.inner.state.read().await;
        if current_state == TunnelState::Running || current_state == TunnelState::Starting {
            let pid = self.inner.pid.read().await.unwrap_or(0);
            return Err(TunnelSupervisorError::AlreadyRunning(pid));
        }

        *self.inner.state.write().await = TunnelState::Starting;
        *self.inner.restart_count.write().await = 0;

        let config = self.inner.config.read().await.clone();
        if config.command_override.is_none() {
            let bin_config = BinaryManagerConfig {
                sidecar_dir: config.sidecar_dir.clone(),
                custom_path: config.custom_binary_path.clone(),
                allow_auto_download: config.allow_auto_download,
                ..Default::default()
            };
            let bin_mgr = CloudflaredBinaryManager::new(bin_config);
            let bin_info = bin_mgr
                .resolve()
                .await
                .map_err(TunnelSupervisorError::BinaryResolution)?;
            *self.inner.resolved_binary.write().await = Some(bin_info.path);
        }

        let _ = self.inner.event_tx.send(TunnelEvent::Starting {
            target_url: config.target_url.clone(),
        });

        let inner_clone = self.inner.clone();
        let mut shutdown_rx = self.inner.shutdown_tx.subscribe();

        let handle = tokio::spawn(async move {
            inner_clone.run_supervision_loop(&mut shutdown_rx).await;
        });

        *self.inner.supervisor_handle.write().await = Some(handle);
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), TunnelSupervisorError> {
        let current_state = *self.inner.state.read().await;
        if current_state == TunnelState::Stopped {
            return Ok(());
        }

        let _ = self.inner.shutdown_tx.send(());

        if let Some(handle) = self.inner.supervisor_handle.write().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }

        *self.inner.state.write().await = TunnelState::Stopped;
        *self.inner.pid.write().await = None;
        *self.inner.started_at.write().await = None;

        let _ = self.inner.event_tx.send(TunnelEvent::Stopped);
        Ok(())
    }

    pub async fn restart(&self) -> Result<(), TunnelSupervisorError> {
        self.stop().await?;
        self.start().await?;
        Ok(())
    }
}

impl SupervisorInner {
    async fn run_supervision_loop(&self, shutdown_rx: &mut broadcast::Receiver<()>) {
        loop {
            let config = self.config.read().await.clone();

            let (program, args) = if let Some(ref override_cmd) = config.command_override {
                if override_cmd.is_empty() {
                    *self.state.write().await = TunnelState::Failed;
                    let _ = self.event_tx.send(TunnelEvent::Failed {
                        reason: "command_override is empty".to_string(),
                    });
                    break;
                }
                (
                    PathBuf::from(&override_cmd[0]),
                    override_cmd[1..].to_vec(),
                )
            } else {
                let bin_path = self
                    .resolved_binary
                    .read()
                    .await
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("cloudflared"));
                let mut args = Vec::new();
                if let Some(ref token) = config.tunnel_token {
                    args.push("tunnel".to_string());
                    args.push("run".to_string());
                    args.push("--token".to_string());
                    args.push(token.clone());
                } else {
                    args.push("tunnel".to_string());
                    args.push("--url".to_string());
                    args.push(config.target_url.clone());
                    args.push("--no-autoupdate".to_string());
                }
                args.extend(config.extra_args.clone());
                (bin_path, args)
            };

            let mut cmd = tokio::process::Command::new(&program);
            cmd.args(&args)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    *self.state.write().await = TunnelState::Failed;
                    let _ = self.event_tx.send(TunnelEvent::Failed {
                        reason: format!("Failed to spawn {}: {}", program.display(), e),
                    });
                    break;
                }
            };

            let pid = child.id().unwrap_or(0);
            *self.pid.write().await = Some(pid);
            *self.state.write().await = TunnelState::Running;
            *self.started_at.write().await = Some(Instant::now());

            let _ = self.event_tx.send(TunnelEvent::Running { pid });

            // Stream stdout and stderr
            if let Some(stdout) = child.stdout.take() {
                let out_tx = self.output_tx.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    let mut reader = tokio::io::BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        let _ = out_tx.send(line);
                    }
                });
            }

            if let Some(stderr) = child.stderr.take() {
                let out_tx = self.output_tx.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    let mut reader = tokio::io::BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        let _ = out_tx.send(line);
                    }
                });
            }

            tokio::select! {
                _ = shutdown_rx.recv() => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    *self.pid.write().await = None;
                    *self.state.write().await = TunnelState::Stopped;
                    break;
                }
                status_res = child.wait() => {
                    let exit_code = status_res.ok().and_then(|s| s.code());
                    *self.last_exit_code.write().await = exit_code;
                    *self.pid.write().await = None;

                    let uptime_secs = self
                        .started_at
                        .read()
                        .await
                        .map(|t| t.elapsed().as_secs())
                        .unwrap_or(0);

                    if uptime_secs >= config.stable_uptime_threshold_secs {
                        *self.restart_count.write().await = 0;
                    }

                    let cur_retries = *self.restart_count.read().await;

                    if config.auto_restart && cur_retries < config.max_retries {
                        let shift = cur_retries.min(30);
                        let backoff = std::cmp::min(
                            config.initial_backoff_secs.saturating_mul(1 << shift),
                            config.max_backoff_secs,
                        );
                        *self.restart_count.write().await = cur_retries + 1;
                        *self.state.write().await = TunnelState::Restarting;

                        let _ = self.event_tx.send(TunnelEvent::UnexpectedExit {
                            exit_code,
                            retry_count: cur_retries + 1,
                            backoff_secs: backoff,
                        });
                        let _ = self.event_tx.send(TunnelEvent::Restarting {
                            retry_count: cur_retries + 1,
                        });

                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {
                                // proceed with auto-restart
                            }
                            _ = shutdown_rx.recv() => {
                                *self.state.write().await = TunnelState::Stopped;
                                break;
                            }
                        }
                    } else {
                        *self.state.write().await = TunnelState::Failed;
                        let _ = self.event_tx.send(TunnelEvent::Failed {
                            reason: format!(
                                "Process exited with code {:?} (retries exhausted: {}/{})",
                                exit_code, cur_retries, config.max_retries
                            ),
                        });
                        break;
                    }
                }
            }
        }
    }
}
