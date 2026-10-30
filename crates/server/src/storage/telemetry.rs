use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static GLOBAL_TELEMETRY: OnceLock<StorageTelemetryEngine> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageTelemetrySnapshot {
    pub active_stream_count: u64,
    pub total_streamed_bytes: u64,
    pub total_uploaded_bytes: u64,
    pub avg_read_latency_micros: u64,
    pub avg_write_latency_micros: u64,
    pub last_read_latency_micros: u64,
    pub last_write_latency_micros: u64,
    pub total_read_ops: u64,
    pub total_write_ops: u64,
    pub disk_bus_read_bytes_per_sec: u64,
    pub disk_bus_write_bytes_per_sec: u64,
    pub disk_bus_utilization_pct: f32,
    pub timestamp_utc: String,
}

pub struct StreamLease {
    engine: &'static StorageTelemetryEngine,
}

impl Drop for StreamLease {
    fn drop(&mut self) {
        self.engine.active_client_streams.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct StorageTelemetryEngine {
    active_client_streams: AtomicU64,
    total_streamed_bytes: AtomicU64,
    total_uploaded_bytes: AtomicU64,
    total_read_operations: AtomicU64,
    total_write_operations: AtomicU64,
    total_read_latency_micros: AtomicU64,
    total_write_latency_micros: AtomicU64,
    last_read_latency_micros: AtomicU64,
    last_write_latency_micros: AtomicU64,
    last_sample_instant: Mutex<Instant>,
    last_sample_read_bytes: AtomicU64,
    last_sample_write_bytes: AtomicU64,
}

impl Default for StorageTelemetryEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageTelemetryEngine {
    pub fn new() -> Self {
        Self {
            active_client_streams: AtomicU64::new(0),
            total_streamed_bytes: AtomicU64::new(0),
            total_uploaded_bytes: AtomicU64::new(0),
            total_read_operations: AtomicU64::new(0),
            total_write_operations: AtomicU64::new(0),
            total_read_latency_micros: AtomicU64::new(0),
            total_write_latency_micros: AtomicU64::new(0),
            last_read_latency_micros: AtomicU64::new(0),
            last_write_latency_micros: AtomicU64::new(0),
            last_sample_instant: Mutex::new(Instant::now()),
            last_sample_read_bytes: AtomicU64::new(0),
            last_sample_write_bytes: AtomicU64::new(0),
        }
    }

    pub fn global() -> &'static StorageTelemetryEngine {
        GLOBAL_TELEMETRY.get_or_init(StorageTelemetryEngine::new)
    }

    /// Acquires an active stream lease, automatically decrementing active count on drop
    pub fn acquire_stream_lease(&'static self) -> StreamLease {
        self.active_client_streams.fetch_add(1, Ordering::SeqCst);
        StreamLease { engine: self }
    }

    /// Records a read operation with byte count and measured latency in microseconds
    pub fn record_read(&self, bytes: u64, elapsed_micros: u64) {
        self.total_streamed_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.total_read_operations.fetch_add(1, Ordering::Relaxed);
        self.total_read_latency_micros.fetch_add(elapsed_micros, Ordering::Relaxed);
        self.last_read_latency_micros.store(elapsed_micros, Ordering::Relaxed);
    }

    /// Records a write operation with byte count and measured latency in microseconds
    pub fn record_write(&self, bytes: u64, elapsed_micros: u64) {
        self.total_uploaded_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.total_write_operations.fetch_add(1, Ordering::Relaxed);
        self.total_write_latency_micros.fetch_add(elapsed_micros, Ordering::Relaxed);
        self.last_write_latency_micros.store(elapsed_micros, Ordering::Relaxed);
    }

    /// Returns the current active client stream count
    pub fn active_stream_count(&self) -> u64 {
        self.active_client_streams.load(Ordering::SeqCst)
    }

    /// Generates an immutable snapshot of storage telemetry, computing throughput and bus load
    pub fn snapshot(&self) -> StorageTelemetrySnapshot {
        let active_streams = self.active_client_streams.load(Ordering::SeqCst);
        let streamed_bytes = self.total_streamed_bytes.load(Ordering::Relaxed);
        let uploaded_bytes = self.total_uploaded_bytes.load(Ordering::Relaxed);
        let read_ops = self.total_read_operations.load(Ordering::Relaxed);
        let write_ops = self.total_write_operations.load(Ordering::Relaxed);
        let total_read_latency = self.total_read_latency_micros.load(Ordering::Relaxed);
        let total_write_latency = self.total_write_latency_micros.load(Ordering::Relaxed);
        let last_read_lat = self.last_read_latency_micros.load(Ordering::Relaxed);
        let last_write_lat = self.last_write_latency_micros.load(Ordering::Relaxed);

        let avg_read_lat = if read_ops > 0 {
            total_read_latency / read_ops
        } else {
            0
        };

        let avg_write_lat = if write_ops > 0 {
            total_write_latency / write_ops
        } else {
            0
        };

        // Calculate rate of disk bus byte transfer
        let (read_rate, write_rate) = {
            let mut last_instant = self.last_sample_instant.lock().unwrap();
            let elapsed_sec = last_instant.elapsed().as_secs_f64();

            let prev_read = self.last_sample_read_bytes.swap(streamed_bytes, Ordering::Relaxed);
            let prev_write = self.last_sample_write_bytes.swap(uploaded_bytes, Ordering::Relaxed);

            let r_rate = if elapsed_sec > 0.05 && streamed_bytes >= prev_read {
                ((streamed_bytes - prev_read) as f64 / elapsed_sec) as u64
            } else {
                0
            };

            let w_rate = if elapsed_sec > 0.05 && uploaded_bytes >= prev_write {
                ((uploaded_bytes - prev_write) as f64 / elapsed_sec) as u64
            } else {
                0
            };

            *last_instant = Instant::now();
            (r_rate, w_rate)
        };

        // Baseline standard SATA/NVMe bus bandwidth capacity (~550 MB/s typical SATA SSD cap)
        const BUS_NOMINAL_BANDWIDTH_BPS: f64 = 550.0 * 1024.0 * 1024.0;
        let combined_rate = (read_rate + write_rate) as f64;
        let bus_utilization_pct = ((combined_rate / BUS_NOMINAL_BANDWIDTH_BPS) * 100.0)
            .min(100.0) as f32;

        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        StorageTelemetrySnapshot {
            active_stream_count: active_streams,
            total_streamed_bytes: streamed_bytes,
            total_uploaded_bytes: uploaded_bytes,
            avg_read_latency_micros: avg_read_lat,
            avg_write_latency_micros: avg_write_lat,
            last_read_latency_micros: last_read_lat,
            last_write_latency_micros: last_write_lat,
            total_read_ops: read_ops,
            total_write_ops: write_ops,
            disk_bus_read_bytes_per_sec: read_rate,
            disk_bus_write_bytes_per_sec: write_rate,
            disk_bus_utilization_pct: bus_utilization_pct,
            timestamp_utc: format!("UTC-EPOCH-{}", now_epoch),
        }
    }
}
