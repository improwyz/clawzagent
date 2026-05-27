//! Container metrics — real-time environmental awareness for the
//! [`RealityModel`](super::RealityModel).
//!
//! [`ContainerMetrics`] fetches live CPU and memory utilisation, container
//! health, and queue depth (advertised via a Docker label) by polling the
//! local Docker daemon through the Bollard API. When Docker is unavailable
//! or the call fails, the struct degrades gracefully to zeroed metrics via
//! [`ContainerMetrics::unavailable`] — collection never panics the runtime.

use std::time::SystemTime;

use bollard::Docker;
use bollard::container::{InspectContainerOptions, StatsOptions};
use chrono::{DateTime, TimeZone, Utc};
use clawz_core::error::ClawzError;
use futures_util::StreamExt;

/// Bytes per megabyte (used for memory unit conversion).
const BYTES_PER_MB: u64 = 1024 * 1024;

/// Docker label that an agent container can set to advertise the number of
/// pending tasks in its local work queue. Populated into
/// [`ContainerMetrics::queue_depth`] when present and parseable.
pub const QUEUE_DEPTH_LABEL: &str = "clawz-queue-depth";

/// Real-time container-level metrics sourced from the Docker (Bollard) API.
///
/// Use [`ContainerMetrics::fetch`] to populate a snapshot for a given
/// container. On any failure path (Docker unreachable, container missing,
/// stats stream empty) the snapshot is replaced with a zeroed value from
/// [`ContainerMetrics::unavailable`] — callers must never block on Docker
/// being available.
#[derive(Debug, Clone)]
pub struct ContainerMetrics {
    /// Docker container ID that these metrics describe.
    pub container_id: String,
    /// Estimated CPU utilisation as a percentage of one core (0.0 - 100.0+).
    pub cpu_percent: f32,
    /// Memory used / limit, in the range `[0.0, 1.0]`.
    pub memory_percent: f32,
    /// Resident memory in megabytes.
    pub memory_used_mb: u64,
    /// Container memory limit in megabytes (0 when unbounded).
    pub memory_limit_mb: u64,
    /// Pending tasks reported via the [`QUEUE_DEPTH_LABEL`] label.
    pub queue_depth: usize,
    /// Container healthcheck status — one of `"healthy"`, `"degraded"`,
    /// `"unhealthy"`, or `"unknown"`.
    pub health_status: String,
    /// Seconds elapsed since the container's last reported start/heartbeat.
    pub last_heartbeat_secs_ago: u64,
    /// When this snapshot was taken.
    pub timestamp: DateTime<Utc>,
}

impl ContainerMetrics {
    /// Returns a zeroed metrics struct — used when collection fails
    /// gracefully (Docker daemon down, container removed, etc.).
    pub fn unavailable(container_id: &str) -> Self {
        Self {
            container_id: container_id.to_string(),
            cpu_percent: 0.0,
            memory_percent: 0.0,
            memory_used_mb: 0,
            memory_limit_mb: 0,
            queue_depth: 0,
            health_status: "unknown".into(),
            last_heartbeat_secs_ago: 0,
            timestamp: Utc::now(),
        }
    }

    /// Poll Bollard for stats on `container_id`.
    ///
    /// On any error path — Docker unreachable, container missing, empty
    /// stats stream — returns a zeroed [`ContainerMetrics::unavailable`]
    /// snapshot rather than propagating. The `Result` is retained for
    /// future signatures that may want to surface diagnostic detail.
    pub async fn fetch(container_id: &str) -> Result<Self, ClawzError> {
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => return Ok(Self::unavailable(container_id)),
        };

        // ── 1. Live CPU & memory via /containers/{id}/stats (one shot) ───────
        let stats_opts = StatsOptions {
            stream: false,
            one_shot: true,
        };
        let mut stats_stream = docker.stats(container_id, Some(stats_opts));
        let stats = match stats_stream.next().await {
            Some(Ok(s)) => s,
            _ => return Ok(Self::unavailable(container_id)),
        };

        let (cpu_percent, memory_used_mb, memory_limit_mb, memory_percent) =
            extract_resource_metrics(&stats);

        // ── 2. Health status, started_at, and queue-depth label via inspect ─
        let (health_status, last_heartbeat_secs_ago, queue_depth) = match docker
            .inspect_container(container_id, None::<InspectContainerOptions>)
            .await
        {
            Ok(info) => {
                let health_status = info
                    .state
                    .as_ref()
                    .and_then(|s| s.health.as_ref())
                    .and_then(|h| h.status)
                    .map(map_health_status)
                    .unwrap_or_else(|| {
                        // No healthcheck — fall back to Running flag.
                        let running = info.state.as_ref().and_then(|s| s.running).unwrap_or(false);
                        if running {
                            "healthy".into()
                        } else {
                            "unhealthy".into()
                        }
                    });

                let last_heartbeat_secs_ago = info
                    .state
                    .as_ref()
                    .and_then(|s| s.started_at.as_ref())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| seconds_since(dt.with_timezone(&Utc)))
                    .unwrap_or(0);

                let queue_depth = info
                    .config
                    .as_ref()
                    .and_then(|c| c.labels.as_ref())
                    .and_then(|l| l.get(QUEUE_DEPTH_LABEL))
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0);

                (health_status, last_heartbeat_secs_ago, queue_depth)
            }
            Err(_) => ("unknown".into(), 0u64, 0usize),
        };

        Ok(Self {
            container_id: container_id.to_string(),
            cpu_percent,
            memory_percent,
            memory_used_mb,
            memory_limit_mb,
            queue_depth,
            health_status,
            last_heartbeat_secs_ago,
            timestamp: Utc::now(),
        })
    }

    /// True when the snapshot reflects a live, healthy collection cycle.
    ///
    /// Used by [`super::RealityModel::build`] to decide whether to mark the
    /// resulting `ContextBundle` with high confidence.
    pub fn is_live(&self) -> bool {
        self.health_status != "unknown" && self.memory_limit_mb > 0
    }
}

/// Compute CPU% (relative to one core) and memory usage / limit / ratio
/// from a Bollard [`Stats`] snapshot.
///
/// Docker reports raw nanosecond counters; we apply the canonical
/// `(delta_container / delta_system) * online_cpus * 100` formula used by
/// `docker stats`.
fn extract_resource_metrics(stats: &bollard::container::Stats) -> (f32, u64, u64, f32) {
    let cpu_delta = stats
        .cpu_stats
        .cpu_usage
        .total_usage
        .saturating_sub(stats.precpu_stats.cpu_usage.total_usage);
    let system_delta = stats
        .cpu_stats
        .system_cpu_usage
        .unwrap_or(0)
        .saturating_sub(stats.precpu_stats.system_cpu_usage.unwrap_or(0));
    let online_cpus = stats
        .cpu_stats
        .online_cpus
        .or(stats.precpu_stats.online_cpus)
        .unwrap_or(1)
        .max(1);

    let cpu_percent = if system_delta > 0 && cpu_delta > 0 {
        ((cpu_delta as f64 / system_delta as f64) * online_cpus as f64 * 100.0) as f32
    } else {
        0.0
    };

    let memory_used = stats.memory_stats.usage.unwrap_or(0);
    let memory_limit = stats.memory_stats.limit.unwrap_or(0);
    let memory_used_mb = memory_used / BYTES_PER_MB;
    let memory_limit_mb = memory_limit / BYTES_PER_MB;
    let memory_percent = if memory_limit > 0 {
        (memory_used as f64 / memory_limit as f64) as f32
    } else {
        0.0
    };

    (cpu_percent, memory_used_mb, memory_limit_mb, memory_percent)
}

/// Map a Bollard health status enum into the canonical ClawZ string set
/// used by [`ContainerMetrics::health_status`].
fn map_health_status(status: bollard::models::HealthStatusEnum) -> String {
    use bollard::models::HealthStatusEnum::*;
    match status {
        HEALTHY => "healthy",
        STARTING => "degraded",
        UNHEALTHY => "unhealthy",
        NONE | EMPTY => "unknown",
    }
    .to_string()
}

/// Seconds between `at` and `now`, saturating at 0.
fn seconds_since(at: DateTime<Utc>) -> u64 {
    let now = Utc::now();
    if now > at {
        (now - at).num_seconds().max(0) as u64
    } else {
        0
    }
}

/// Internal helper retained so SystemTime isn't an unused import in stripped
/// builds; the public API uses [`chrono::Utc::now`] directly.
#[doc(hidden)]
#[allow(dead_code)]
fn _systemtime_now() -> SystemTime {
    SystemTime::now()
}

/// Compatibility re-export so callers can write
/// `use crate::reality::container_metrics::Utc` if needed.
#[doc(hidden)]
pub use chrono::Utc as _UtcReexport;

/// Inline placeholder to allow timezone construction in tests without
/// pulling chrono into scope.
#[doc(hidden)]
pub fn _epoch() -> DateTime<Utc> {
    Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_metrics_unavailable_is_zeroed() {
        let m = ContainerMetrics::unavailable("abc123");
        assert_eq!(m.container_id, "abc123");
        assert_eq!(m.cpu_percent, 0.0);
        assert_eq!(m.memory_percent, 0.0);
        assert_eq!(m.memory_used_mb, 0);
        assert_eq!(m.memory_limit_mb, 0);
        assert_eq!(m.queue_depth, 0);
        assert_eq!(m.health_status, "unknown");
        assert_eq!(m.last_heartbeat_secs_ago, 0);
        assert!(!m.is_live());
    }

    #[test]
    fn container_metrics_is_live_requires_known_health_and_limit() {
        let mut m = ContainerMetrics::unavailable("c1");
        m.health_status = "healthy".into();
        // Still not live — memory_limit_mb == 0
        assert!(!m.is_live());
        m.memory_limit_mb = 512;
        assert!(m.is_live());
    }

    #[tokio::test]
    async fn container_metrics_fetch_returns_unavailable_when_no_docker() {
        // Targets a container ID that almost certainly does not exist; the
        // fetch must not panic and must return a zeroed snapshot regardless
        // of whether Docker is reachable in this environment.
        let m = ContainerMetrics::fetch("clawz-nonexistent-container-zzz")
            .await
            .expect("fetch never errors — it degrades gracefully");
        assert_eq!(m.container_id, "clawz-nonexistent-container-zzz");
        // Either Docker isn't installed (unavailable -> "unknown") or the
        // container doesn't exist (inspect failed -> "unknown") — either
        // way the snapshot must not assert liveness.
        assert!(!m.is_live());
    }

    #[test]
    fn epoch_helper_is_stable() {
        let e = _epoch();
        assert_eq!(e.timestamp(), 0);
    }
}
