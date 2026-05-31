//! Prometheus-compatible metrics primitives and domain-specific recorders.
//!
//! `MetricsRegistry` holds thread-safe counters, gauges, and histograms.
//! It is instantiated once in the worker and once in the gateway, then
//! passed to every subsystem that needs to emit metrics.
//!
//! The `export_prometheus()` method renders the full registry as a text
//! string suitable for scraping by Prometheus or VictoriaMetrics.
//!
//! // Dependency: used by worker::scheduler, worker::provider_router,
//! // gateway::handlers, and all domain recorders below.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

// ── Primitive counters / gauges ────────────────────────────────────────────────

/// An atomically-incrementing counter.
/// // Used by: MetricsRegistry::counter, record_request
#[derive(Debug, Default)]
pub struct Counter {
    value: AtomicU64,
}

impl Counter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment by 1.
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment by `n`.
    pub fn add(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.value.store(0, Ordering::Relaxed);
    }
}

/// A simple floating-point gauge (last-value semantics).
///
/// Internally stores the f64 as raw bits inside an AtomicU64 so we can
/// update without a mutex on the hot path.
/// // Used by: MetricsRegistry::gauge
#[derive(Debug, Default)]
pub struct Gauge {
    bits: AtomicU64,
}

impl Gauge {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, v: f64) {
        self.bits.store(v.to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self) -> f64 {
        f64::from_bits(self.bits.load(Ordering::Relaxed))
    }
}

/// A histogram that tracks count, sum and a simple set of fixed buckets.
///
/// Designed for latency metrics. Buckets are fixed at creation time and
/// rendered in Prometheus text format via `prometheus_lines()`.
/// // Used by: MetricsRegistry::histogram, record_request
#[derive(Debug)]
pub struct Histogram {
    /// Upper bounds of the buckets in milliseconds.
    buckets: Vec<f64>,
    counts: Vec<AtomicU64>,
    total_count: AtomicU64,
    total_sum: parking_lot::Mutex<f64>,
}

impl Histogram {
    /// Create with the given upper bounds (must be sorted ascending).
    pub fn new(buckets: Vec<f64>) -> Self {
        let n = buckets.len() + 1; // +1 for the overflow (+Inf) bucket
        let counts = (0..n).map(|_| AtomicU64::new(0)).collect();
        Self {
            buckets,
            counts,
            total_count: AtomicU64::new(0),
            total_sum: parking_lot::Mutex::new(0.0),
        }
    }

    /// Standard web/latency bucket boundaries in milliseconds.
    pub fn latency_buckets() -> Self {
        Self::new(vec![
            1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 5000.0,
        ])
    }

    /// Record a single observation into the appropriate bucket.
    pub fn observe(&self, value: f64) {
        self.total_count.fetch_add(1, Ordering::Relaxed);
        *self.total_sum.lock() += value;

        for (i, &bound) in self.buckets.iter().enumerate() {
            if value <= bound {
                self.counts[i].fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        // overflow bucket
        self.counts[self.buckets.len()].fetch_add(1, Ordering::Relaxed);
    }

    pub fn count(&self) -> u64 {
        self.total_count.load(Ordering::Relaxed)
    }

    pub fn sum(&self) -> f64 {
        *self.total_sum.lock()
    }

    pub fn mean(&self) -> f64 {
        let c = self.count();
        if c == 0 {
            0.0
        } else {
            self.sum() / c as f64
        }
    }

    /// Render as Prometheus text format lines.
    pub fn prometheus_lines(&self, name: &str, labels: &str) -> String {
        let mut out = String::new();
        let lbl = if labels.is_empty() {
            String::new()
        } else {
            format!("{{{labels}}}")
        };

        let mut running = 0u64;
        for (i, &bound) in self.buckets.iter().enumerate() {
            running += self.counts[i].load(Ordering::Relaxed);
            out.push_str(&format!("{name}_bucket{lbl}{{le=\"{bound}\"}} {running}\n"));
        }
        running += self.counts[self.buckets.len()].load(Ordering::Relaxed);
        out.push_str(&format!("{name}_bucket{lbl}{{le=\"+Inf\"}} {running}\n"));
        out.push_str(&format!("{name}_count{lbl} {}\n", self.count()));
        out.push_str(&format!("{name}_sum{lbl} {}\n", self.sum()));
        out
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Self::latency_buckets()
    }
}

// ── MetricsRegistry ───────────────────────────────────────────────────────────

/// Thread-safe metrics registry backed by `parking_lot::RwLock`.
///
/// Counters, gauges, and histograms are created lazily on first access
/// and then cached for fast subsequent lookups.
/// // Used by: worker::bootstrap, gateway::bootstrap
#[derive(Debug, Default, Clone)]
pub struct MetricsRegistry {
    inner: Arc<RegistryInner>,
}

#[derive(Debug, Default)]
struct RegistryInner {
    counters: RwLock<HashMap<String, Arc<Counter>>>,
    gauges: RwLock<HashMap<String, Arc<Gauge>>>,
    histograms: RwLock<HashMap<String, Arc<Histogram>>>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Counter helpers ───────────────────────────────────────────────────────

    /// Get (or create) a counter by name.
    pub fn counter(&self, name: &str) -> Arc<Counter> {
        let read = self.inner.counters.read();
        if let Some(c) = read.get(name) {
            return Arc::clone(c);
        }
        drop(read);
        let mut write = self.inner.counters.write();
        write
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(Counter::new()))
            .clone()
    }

    pub fn inc(&self, name: &str) {
        self.counter(name).inc();
    }

    pub fn add(&self, name: &str, n: u64) {
        self.counter(name).add(n);
    }

    // ── Gauge helpers ─────────────────────────────────────────────────────────

    /// Get (or create) a gauge by name.
    pub fn gauge(&self, name: &str) -> Arc<Gauge> {
        let read = self.inner.gauges.read();
        if let Some(g) = read.get(name) {
            return Arc::clone(g);
        }
        drop(read);
        let mut write = self.inner.gauges.write();
        write
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(Gauge::new()))
            .clone()
    }

    pub fn set_gauge(&self, name: &str, value: f64) {
        self.gauge(name).set(value);
    }

    // ── Histogram helpers ─────────────────────────────────────────────────────

    /// Get (or create) a histogram by name.
    pub fn histogram(&self, name: &str) -> Arc<Histogram> {
        let read = self.inner.histograms.read();
        if let Some(h) = read.get(name) {
            return Arc::clone(h);
        }
        drop(read);
        let mut write = self.inner.histograms.write();
        write
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(Histogram::default()))
            .clone()
    }

    pub fn observe(&self, name: &str, value: f64) {
        self.histogram(name).observe(value);
    }

    // ── Prometheus export ─────────────────────────────────────────────────────

    /// Render the entire registry in Prometheus exposition format.
    pub fn export_prometheus(&self) -> String {
        let mut out = String::new();

        for (name, counter) in self.inner.counters.read().iter() {
            out.push_str(&format!("# TYPE {name} counter\n"));
            out.push_str(&format!("{name} {}\n", counter.get()));
        }

        for (name, gauge) in self.inner.gauges.read().iter() {
            out.push_str(&format!("# TYPE {name} gauge\n"));
            out.push_str(&format!("{name} {}\n", gauge.get()));
        }

        for (name, hist) in self.inner.histograms.read().iter() {
            out.push_str(&format!("# TYPE {name} histogram\n"));
            out.push_str(&hist.prometheus_lines(name, ""));
        }

        out
    }
}

// ── Domain-specific recording functions ───────────────────────────────────────

/// Record an LLM provider request.
/// // Called by: worker::provider implementations after each chat() call.
pub fn record_request(
    registry: &MetricsRegistry,
    provider: &str,
    model: &str,
    latency_ms: f64,
    tokens: u64,
) {
    registry.inc("clawz_requests_total");
    registry.add("clawz_tokens_total", tokens);
    registry.observe("clawz_request_latency_ms", latency_ms);

    debug!(
        provider = provider,
        model = model,
        latency_ms = latency_ms,
        tokens = tokens,
        "provider request"
    );
}

/// Record a tool execution.
/// // Called by: worker::tool_orchestrator after each tool run.
pub fn record_tool_execution(
    registry: &MetricsRegistry,
    tool_name: &str,
    duration_ms: f64,
    success: bool,
) {
    registry.inc("clawz_tool_executions_total");
    registry.observe("clawz_tool_duration_ms", duration_ms);
    if success {
        registry.inc("clawz_tool_successes_total");
    } else {
        registry.inc("clawz_tool_failures_total");
    }

    debug!(
        tool = tool_name,
        duration_ms = duration_ms,
        success = success,
        "tool execution"
    );
}

/// Record a channel message (direction: "in" or "out").
/// // Called by: worker::channel_plugin implementations on send/receive.
pub fn record_channel_message(registry: &MetricsRegistry, channel: &str, direction: &str) {
    let key = format!("clawz_channel_messages_{direction}");
    registry.inc(&key);
    debug!(channel = channel, direction = direction, "channel message");
}

/// Record a governance policy evaluation.
/// // Called by: worker::governance_engine after each evaluate() call.
pub fn record_governance_check(
    registry: &MetricsRegistry,
    result: &str, // "allowed" | "denied" | "review"
    latency_ms: f64,
) {
    registry.inc("clawz_governance_checks_total");
    let key = format!("clawz_governance_{result}_total");
    registry.inc(&key);
    registry.observe("clawz_governance_latency_ms", latency_ms);

    info!(result = result, latency_ms = latency_ms, "governance check");
}

/// Record a deployment operation.
/// // Called by: worker::deploy adapters on create/stop/destroy.
pub fn record_deploy_event(registry: &MetricsRegistry, provider: &str, event: &str) {
    registry.inc("clawz_deploy_events_total");
    debug!(provider = provider, event = event, "deploy event");
}

/// Record an error by subsystem.
/// // Called by: any subsystem that catches an error it wants to track.
pub fn record_error(registry: &MetricsRegistry, subsystem: &str) {
    let key = format!("clawz_{subsystem}_errors_total");
    registry.inc(&key);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerfDimension {
    Accuracy,
    Speed,
    Cost,
    Reliability,
    Satisfaction,
    GoalAlignment,
}

impl PerfDimension {
    pub const ALL: [PerfDimension; 6] = [
        PerfDimension::Accuracy,
        PerfDimension::Speed,
        PerfDimension::Cost,
        PerfDimension::Reliability,
        PerfDimension::Satisfaction,
        PerfDimension::GoalAlignment,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            PerfDimension::Accuracy => "Accuracy",
            PerfDimension::Speed => "Speed",
            PerfDimension::Cost => "Cost",
            PerfDimension::Reliability => "Reliability",
            PerfDimension::Satisfaction => "Satisfaction",
            PerfDimension::GoalAlignment => "Goal Alignment",
        }
    }
}
