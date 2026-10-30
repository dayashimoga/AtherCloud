use crate::tunnel::supervisor::{CloudflaredSupervisor, TunnelState};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

#[derive(Debug, thiserror::Error)]
pub enum HeartbeatError {
    #[error("Heartbeat worker is already running")]
    AlreadyRunning,
    #[error("Supervisor error during restart: {0}")]
    SupervisorError(String),
    #[error("Network probe error: {0}")]
    NetworkError(#[from] reqwest::Error),
    #[error("Worker task aborted")]
    TaskAborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelHealthState {
    Healthy,
    Degraded,
    Unhealthy,
    Paused,
    Recovering,
}

impl TunnelHealthState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TunnelHealthState::Healthy => "Healthy",
            TunnelHealthState::Degraded => "Degraded",
            TunnelHealthState::Unhealthy => "Unhealthy",
            TunnelHealthState::Paused => "Paused",
            TunnelHealthState::Recovering => "Recovering",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelHeartbeatMetrics {
    pub state: TunnelHealthState,
    pub last_ping_at_epoch_ms: Option<u64>,
    pub last_latency_ms: Option<f64>,
    pub avg_latency_ms: Option<f64>,
    pub min_latency_ms: Option<f64>,
    pub max_latency_ms: Option<f64>,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
    pub total_pings: u64,
    pub total_failures: u64,
    pub total_restarts_triggered: u32,
    pub last_error: Option<String>,
    pub last_healthy_epoch_ms: Option<u64>,
}

impl Default for TunnelHeartbeatMetrics {
    fn default() -> Self {
        Self {
            state: TunnelHealthState::Paused,
            last_ping_at_epoch_ms: None,
            last_latency_ms: None,
            avg_latency_ms: None,
            min_latency_ms: None,
            max_latency_ms: None,
            consecutive_failures: 0,
            consecutive_successes: 0,
            total_pings: 0,
            total_failures: 0,
            total_restarts_triggered: 0,
            last_error: None,
            last_healthy_epoch_ms: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HeartbeatEvent {
    PingSuccess {
        latency_ms: f64,
        total_pings: u64,
    },
    PingFailure {
        error: String,
        consecutive_failures: u32,
        will_restart: bool,
    },
    HealthTransition {
        old_state: TunnelHealthState,
        new_state: TunnelHealthState,
    },
    RestartTriggered {
        consecutive_failures: u32,
        total_restarts: u32,
    },
    WorkerStarted,
    WorkerStopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelHeartbeatConfig {
    pub ping_interval: Duration,
    pub timeout: Duration,
    pub max_consecutive_failures: u32,
    pub auto_restart: bool,
    pub restart_cooldown: Duration,
    pub probe_path: String,
    pub probe_override_url: Option<String>,
}

impl Default for TunnelHeartbeatConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(15),
            timeout: Duration::from_secs(5),
            max_consecutive_failures: 3,
            auto_restart: true,
            restart_cooldown: Duration::from_secs(30),
            probe_path: String::new(),
            probe_override_url: None,
        }
    }
}

fn current_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct HeartbeatInner {
    supervisor: CloudflaredSupervisor,
    config: RwLock<TunnelHeartbeatConfig>,
    metrics: RwLock<TunnelHeartbeatMetrics>,
    is_running: AtomicBool,
    event_tx: broadcast::Sender<HeartbeatEvent>,
    shutdown_tx: broadcast::Sender<()>,
    worker_handle: tokio::sync::RwLock<Option<tokio::task::JoinHandle<()>>>,
    http_client: reqwest::Client,
}

#[derive(Clone)]
pub struct TunnelHeartbeatWorker {
    inner: Arc<HeartbeatInner>,
}

impl TunnelHeartbeatWorker {
    pub fn new(supervisor: CloudflaredSupervisor, config: TunnelHeartbeatConfig) -> Self {
        let (event_tx, _) = broadcast::channel(128);
        let (shutdown_tx, _) = broadcast::channel(1);

        let http_client = reqwest::Client::builder()
            .timeout(config.timeout)
            .redirect(reqwest::redirect::Policy::limited(5))
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            inner: Arc::new(HeartbeatInner {
                supervisor,
                config: RwLock::new(config),
                metrics: RwLock::new(TunnelHeartbeatMetrics::default()),
                is_running: AtomicBool::new(false),
                event_tx,
                shutdown_tx,
                worker_handle: tokio::sync::RwLock::new(None),
                http_client,
            }),
        }
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<HeartbeatEvent> {
        self.inner.event_tx.subscribe()
    }

    pub fn config(&self) -> TunnelHeartbeatConfig {
        self.inner.config.read().unwrap().clone()
    }

    pub fn update_config(&self, new_config: TunnelHeartbeatConfig) {
        *self.inner.config.write().unwrap() = new_config;
    }

    pub fn metrics(&self) -> TunnelHeartbeatMetrics {
        self.inner.metrics.read().unwrap().clone()
    }

    pub fn is_healthy(&self) -> bool {
        self.inner.metrics.read().unwrap().state == TunnelHealthState::Healthy
    }

    pub fn is_running(&self) -> bool {
        self.inner.is_running.load(Ordering::SeqCst)
    }

    pub fn supervisor(&self) -> &CloudflaredSupervisor {
        &self.inner.supervisor
    }

    pub fn start(&self) -> Result<(), HeartbeatError> {
        if self.inner.is_running.swap(true, Ordering::SeqCst) {
            return Err(HeartbeatError::AlreadyRunning);
        }

        let inner_clone = self.inner.clone();
        let mut shutdown_rx = self.inner.shutdown_tx.subscribe();

        let _ = self.inner.event_tx.send(HeartbeatEvent::WorkerStarted);

        let handle = tokio::spawn(async move {
            inner_clone.run_monitoring_loop(&mut shutdown_rx).await;
        });

        let inner_for_handle = self.inner.clone();
        tokio::spawn(async move {
            *inner_for_handle.worker_handle.write().await = Some(handle);
        });

        Ok(())
    }

    pub async fn stop(&self) -> Result<(), HeartbeatError> {
        if !self.inner.is_running.swap(false, Ordering::SeqCst) {
            return Ok(());
        }

        let _ = self.inner.shutdown_tx.send(());

        if let Some(handle) = self.inner.worker_handle.write().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), handle).await;
        }

        {
            let mut m = self.inner.metrics.write().unwrap();
            let old_state = m.state;
            m.state = TunnelHealthState::Paused;
            if old_state != TunnelHealthState::Paused {
                let _ = self.inner.event_tx.send(HeartbeatEvent::HealthTransition {
                    old_state,
                    new_state: TunnelHealthState::Paused,
                });
            }
        }

        let _ = self.inner.event_tx.send(HeartbeatEvent::WorkerStopped);
        Ok(())
    }

    pub async fn probe_once(&self) -> Result<f64, String> {
        self.inner.perform_probe().await
    }
}

impl HeartbeatInner {
    async fn determine_probe_target(&self) -> Option<String> {
        let config = self.config.read().unwrap().clone();
        if let Some(override_url) = config.probe_override_url {
            return Some(override_url);
        }

        let pub_url = self.supervisor.public_url().await;
        pub_url.map(|base| {
            if config.probe_path.is_empty() {
                base
            } else if config.probe_path.starts_with('/') {
                format!("{}{}", base.trim_end_matches('/'), config.probe_path)
            } else {
                format!("{}/{}", base.trim_end_matches('/'), config.probe_path)
            }
        })
    }

    async fn perform_probe(&self) -> Result<f64, String> {
        let config = self.config.read().unwrap().clone();
        let target_url = match self.determine_probe_target().await {
            Some(u) => u,
            None => {
                let sup_state = self.supervisor.state().await;
                if sup_state == TunnelState::Stopped {
                    let mut m = self.metrics.write().unwrap();
                    let old_state = m.state;
                    m.state = TunnelHealthState::Paused;
                    if old_state != TunnelHealthState::Paused {
                        let _ = self.event_tx.send(HeartbeatEvent::HealthTransition {
                            old_state,
                            new_state: TunnelHealthState::Paused,
                        });
                    }
                    return Err("Tunnel supervisor is stopped; heartbeat paused".to_string());
                } else {
                    return Err("Tunnel is running but no public ingress URL has been assigned yet".to_string());
                }
            }
        };

        let start = Instant::now();
        let probe_res = self
            .http_client
            .get(&target_url)
            .timeout(config.timeout)
            .send()
            .await;

        let elapsed = start.elapsed();
        let latency_ms = elapsed.as_secs_f64() * 1000.0;
        let now_epoch = current_epoch_ms();

        match probe_res {
            Ok(resp) => {
                let status = resp.status();
                // 5xx Server Error implies tunnel / edge failure
                if status.is_server_error() {
                    let err_msg = format!("HTTP error status: {}", status);
                    self.record_failure(&err_msg, now_epoch).await;
                    Err(err_msg)
                } else {
                    self.record_success(latency_ms, now_epoch);
                    Ok(latency_ms)
                }
            }
            Err(e) => {
                let err_msg = e.to_string();
                self.record_failure(&err_msg, now_epoch).await;
                Err(err_msg)
            }
        }
    }

    fn record_success(&self, latency_ms: f64, now_epoch: u64) {
        let mut m = self.metrics.write().unwrap();
        m.consecutive_failures = 0;
        m.consecutive_successes += 1;
        m.total_pings += 1;
        m.last_latency_ms = Some(latency_ms);
        m.last_ping_at_epoch_ms = Some(now_epoch);
        m.last_healthy_epoch_ms = Some(now_epoch);

        m.min_latency_ms = Some(match m.min_latency_ms {
            Some(min) => min.min(latency_ms),
            None => latency_ms,
        });
        m.max_latency_ms = Some(match m.max_latency_ms {
            Some(max) => max.max(latency_ms),
            None => latency_ms,
        });

        // Cumulative running average
        let successes = m.consecutive_successes as f64;
        m.avg_latency_ms = Some(match m.avg_latency_ms {
            Some(avg) => avg + (latency_ms - avg) / successes,
            None => latency_ms,
        });

        let old_state = m.state;
        m.state = TunnelHealthState::Healthy;
        let total_pings = m.total_pings;
        drop(m);

        if old_state != TunnelHealthState::Healthy {
            let _ = self.event_tx.send(HeartbeatEvent::HealthTransition {
                old_state,
                new_state: TunnelHealthState::Healthy,
            });
        }

        let _ = self.event_tx.send(HeartbeatEvent::PingSuccess {
            latency_ms,
            total_pings,
        });
    }

    async fn record_failure(&self, error: &str, now_epoch: u64) {
        let (consecutive, old_state, new_state, should_restart, total_restarts) = {
            let config = self.config.read().unwrap().clone();
            let mut m = self.metrics.write().unwrap();

            m.consecutive_failures += 1;
            m.consecutive_successes = 0;
            m.total_pings += 1;
            m.total_failures += 1;
            m.last_error = Some(error.to_string());
            m.last_ping_at_epoch_ms = Some(now_epoch);

            let consecutive = m.consecutive_failures;
            let old_state = m.state;

            let is_unhealthy = consecutive >= config.max_consecutive_failures;
            let new_state = if is_unhealthy {
                TunnelHealthState::Unhealthy
            } else {
                TunnelHealthState::Degraded
            };
            m.state = new_state;

            let should_restart = is_unhealthy && config.auto_restart;
            let total_restarts = if should_restart {
                m.total_restarts_triggered += 1;
                m.state = TunnelHealthState::Recovering;
                m.total_restarts_triggered
            } else {
                m.total_restarts_triggered
            };
            (consecutive, old_state, new_state, should_restart, total_restarts)
        };

        if old_state != new_state {
            let _ = self.event_tx.send(HeartbeatEvent::HealthTransition {
                old_state,
                new_state,
            });
        }

        let _ = self.event_tx.send(HeartbeatEvent::PingFailure {
            error: error.to_string(),
            consecutive_failures: consecutive,
            will_restart: should_restart,
        });

        if should_restart {
            let _ = self.event_tx.send(HeartbeatEvent::RestartTriggered {
                consecutive_failures: consecutive,
                total_restarts,
            });

            // Trigger supervisor restart asynchronously
            let _ = self.supervisor.restart().await;
        }
    }

    async fn run_monitoring_loop(
        self: Arc<Self>,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) {
        let interval_duration = self.config.read().unwrap().ping_interval;
        let mut ticker = tokio::time::interval(interval_duration);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // First immediate tick completes immediately
        let _ = ticker.tick().await;

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    break;
                }
                _ = ticker.tick() => {
                    if !self.is_running.load(Ordering::SeqCst) {
                        break;
                    }

                    // Dynamically update interval if config was modified
                    let current_cfg_interval = self.config.read().unwrap().ping_interval;
                    if current_cfg_interval != ticker.period() {
                        ticker = tokio::time::interval(current_cfg_interval);
                        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    }

                    let _ = self.perform_probe().await;
                }
            }
        }
    }
}
