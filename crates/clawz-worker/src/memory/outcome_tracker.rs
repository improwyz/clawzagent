//! Closed-loop outcome tracking for the self-improvement pipeline.
//!
//! [`OutcomeTracker`] records per-task results, watches for consecutive
//! failure spikes, and exposes the live [`PerfDimension`] metrics that the
//! [`ThresholdEvaluator`](crate::memory::improvement::ThresholdEvaluator)
//! consumes to drive self-tuning.
//!
//! ## Wiring (Stage 2 — Task 20)
//!
//! Stage 1 (this module) provides the building block; Stage 2 wires
//! `record()` calls into the worker `run_turn()` loop and threads
//! `current_metrics()` into the [`SelfImprovementLoop`] aggregator.
//!
//! ## Metric Derivation
//!
//! | Dimension      | Formula                                                |
//! |----------------|--------------------------------------------------------|
//! | `Speed`        | `1.0 - (avg_duration_ms / 10_000)` clamped to `[0,1]` |
//! | `Accuracy`     | `success_count / task_count` clamped to `[0,1]`        |
//! | `Reliability`  | `(task_count - failure_count) / task_count`            |
//!
//! `Speed` returns `1.0` until at least one duration sample has been recorded
//! (the tracker has no opinion on speed it has not measured).

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use clawz_core::metrics::PerfDimension;
use tokio::sync::RwLock;

/// Cap on the rolling history of outcomes retained by [`OutcomeTracker`].
const RECENT_CAP: usize = 100;

/// Baseline used to normalise average task latency into the `Speed`
/// metric: any task averaging 10 s is mapped to `Speed = 0`.
const SPEED_BASELINE_MS: f32 = 10_000.0;

/// Outcome signal fed back into the self-improvement pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOutcome {
    Success,
    Failure,
    Timeout,
    Cancelled,
}

impl TaskOutcome {
    /// Whether this outcome counts as a failure for alerting purposes.
    /// `Timeout` is treated as a failure; `Cancelled` is not.
    pub fn is_failure(self) -> bool {
        matches!(self, TaskOutcome::Failure | TaskOutcome::Timeout)
    }

    /// Whether this outcome counts as a success.
    pub fn is_success(self) -> bool {
        matches!(self, TaskOutcome::Success)
    }
}

/// Snapshot of the tracker's live state.
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct OutcomeState {
    pub task_count: usize,
    pub success_count: usize,
    pub failure_count: usize,
    pub timeout_count: usize,
    pub alert_triggered: bool,
    pub last_outcome: Option<TaskOutcome>,
    /// Running average of task durations recorded via
    /// [`OutcomeTracker::record_with_duration`]. `None` until the first
    /// sample is supplied.
    pub avg_duration_ms: Option<f32>,
    /// Consecutive failure run length at the moment of snapshot.
    pub consecutive_failures: usize,
}


/// Records task outcomes, observes consecutive-failure spikes, and
/// produces [`PerfDimension`] metrics for the self-improvement loop.
pub struct OutcomeTracker {
    /// Consecutive-failure threshold that flips `alert_triggered` to true.
    failure_threshold: usize,
    state: RwLock<OutcomeState>,
    recent: RwLock<VecDeque<(DateTime<Utc>, TaskOutcome)>>,
    /// Accumulated duration sum (ms) and sample count, used to compute
    /// `avg_duration_ms` lazily without re-scanning history.
    duration_sum: RwLock<(f64, usize)>,
}

impl OutcomeTracker {
    /// Create a fresh tracker. `failure_threshold` consecutive failures will
    /// set `alert_triggered = true` on the next `record()` call.
    pub fn new(failure_threshold: usize) -> Self {
        Self {
            failure_threshold: failure_threshold.max(1),
            state: RwLock::new(OutcomeState::default()),
            recent: RwLock::new(VecDeque::with_capacity(RECENT_CAP)),
            duration_sum: RwLock::new((0.0, 0)),
        }
    }

    /// Record one task outcome. Returns `true` if this call just transitioned
    /// the tracker into the alert state (i.e. the threshold was crossed on
    /// this very record).
    pub async fn record(&self, task_id: &str, outcome: TaskOutcome) -> bool {
        self.record_inner(task_id, outcome, None).await
    }

    /// Record an outcome together with its measured duration, used to feed
    /// the `Speed` metric. Returns `true` if this call just flipped the
    /// alert on.
    pub async fn record_with_duration(
        &self,
        task_id: &str,
        outcome: TaskOutcome,
        duration_ms: f32,
    ) -> bool {
        self.record_inner(task_id, outcome, Some(duration_ms)).await
    }

    async fn record_inner(
        &self,
        _task_id: &str,
        outcome: TaskOutcome,
        duration_ms: Option<f32>,
    ) -> bool {
        // Append to the rolling window first so callers reading `recent`
        // mid-flight see the freshest entry alongside the freshest state.
        {
            let mut recent = self.recent.write().await;
            if recent.len() == RECENT_CAP {
                recent.pop_front();
            }
            recent.push_back((Utc::now(), outcome));
        }

        // Update the running duration aggregate.
        if let Some(ms) = duration_ms {
            let mut agg = self.duration_sum.write().await;
            agg.0 += ms as f64;
            agg.1 += 1;
        }
        let avg_duration_ms = {
            let agg = self.duration_sum.read().await;
            if agg.1 == 0 {
                None
            } else {
                Some((agg.0 / agg.1 as f64) as f32)
            }
        };

        // Update tallies and detect the alert transition.
        let mut state = self.state.write().await;
        state.task_count += 1;
        state.last_outcome = Some(outcome);
        state.avg_duration_ms = avg_duration_ms;

        match outcome {
            TaskOutcome::Success => {
                state.success_count += 1;
                state.consecutive_failures = 0;
                // A success clears any prior alert: the system is recovering.
                state.alert_triggered = false;
                false
            }
            TaskOutcome::Failure => {
                state.failure_count += 1;
                state.consecutive_failures += 1;
                self.maybe_trigger_alert(&mut state)
            }
            TaskOutcome::Timeout => {
                state.timeout_count += 1;
                state.failure_count += 1;
                state.consecutive_failures += 1;
                self.maybe_trigger_alert(&mut state)
            }
            TaskOutcome::Cancelled => {
                // Cancelled tasks neither succeed nor fail — they do not
                // disturb the consecutive-failure run.
                false
            }
        }
    }

    /// Flip the alert on if the consecutive-failure count just crossed the
    /// configured threshold. Returns `true` only on the leading edge.
    fn maybe_trigger_alert(&self, state: &mut OutcomeState) -> bool {
        if state.consecutive_failures >= self.failure_threshold && !state.alert_triggered {
            state.alert_triggered = true;
            true
        } else {
            false
        }
    }

    /// Current snapshot of outcome state.
    pub async fn state(&self) -> OutcomeState {
        self.state.read().await.clone()
    }

    /// Whether the failure-spike alert is currently latched.
    pub async fn is_alert_triggered(&self) -> bool {
        self.state.read().await.alert_triggered
    }

    /// Return the rolling window of recent outcomes (oldest first, up to
    /// [`RECENT_CAP`] entries).
    pub async fn recent(&self) -> Vec<(DateTime<Utc>, TaskOutcome)> {
        self.recent.read().await.iter().copied().collect()
    }

    /// Manually reset the alert flag (e.g. after operators acknowledge the
    /// spike). Counts and history are preserved.
    pub async fn acknowledge_alert(&self) {
        let mut state = self.state.write().await;
        state.alert_triggered = false;
        state.consecutive_failures = 0;
    }

    /// Current metrics in the format expected by
    /// [`ThresholdEvaluator`](crate::memory::improvement::ThresholdEvaluator).
    ///
    /// Returns three entries — `Speed`, `Accuracy`, `Reliability` — even when
    /// no tasks have been recorded yet, in which case all values are `1.0`.
    pub async fn current_metrics(&self) -> Vec<(PerfDimension, f32)> {
        let state = self.state.read().await;

        let speed = match state.avg_duration_ms {
            None => 1.0,
            Some(avg) => clamp01(1.0 - (avg / SPEED_BASELINE_MS)),
        };

        let (accuracy, reliability) = if state.task_count == 0 {
            (1.0, 1.0)
        } else {
            let total = state.task_count as f32;
            let acc = state.success_count as f32 / total;
            let rel = (state.task_count - state.failure_count) as f32 / total;
            (clamp01(acc), clamp01(rel))
        };

        vec![
            (PerfDimension::Speed, speed),
            (PerfDimension::Accuracy, accuracy),
            (PerfDimension::Reliability, reliability),
        ]
    }
}

#[inline]
fn clamp01(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Integration test — alert fires only after `failure_threshold`
    /// consecutive failures, and is cleared by an intervening success.
    #[tokio::test]
    async fn alert_fires_after_consecutive_failures() {
        let tracker = OutcomeTracker::new(3);

        // Two failures: below threshold, no alert yet.
        assert!(!tracker.record("t1", TaskOutcome::Failure).await);
        assert!(!tracker.record("t2", TaskOutcome::Failure).await);
        assert!(!tracker.is_alert_triggered().await);

        // Third consecutive failure trips the alert — and the return value
        // signals the *leading edge* transition.
        assert!(tracker.record("t3", TaskOutcome::Failure).await);
        assert!(tracker.is_alert_triggered().await);

        // A further failure stays in the alert state but does *not* re-fire.
        assert!(!tracker.record("t4", TaskOutcome::Failure).await);

        // A success clears the alert and resets the consecutive-failure run.
        tracker.record("t5", TaskOutcome::Success).await;
        let state = tracker.state().await;
        assert!(!state.alert_triggered);
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.failure_count, 4);
        assert_eq!(state.success_count, 1);
        assert_eq!(state.task_count, 5);
    }

    /// Timeouts are folded into the failure count and count toward the
    /// consecutive-failure run, while Cancelled outcomes are inert.
    #[tokio::test]
    async fn timeout_counts_as_failure_cancelled_does_not() {
        let tracker = OutcomeTracker::new(2);

        tracker.record("t1", TaskOutcome::Timeout).await;
        // A cancelled task in the middle must not reset the failure run.
        tracker.record("t2", TaskOutcome::Cancelled).await;
        let fired = tracker.record("t3", TaskOutcome::Timeout).await;

        assert!(fired, "second timeout (across a cancelled) must trip the alert");
        let state = tracker.state().await;
        assert_eq!(state.timeout_count, 2);
        assert_eq!(state.failure_count, 2);
        assert_eq!(state.task_count, 3);
        assert_eq!(state.consecutive_failures, 2);
    }

    /// Metric derivation — accuracy and reliability reflect real ratios,
    /// and Speed reacts to recorded durations relative to the 10 s baseline.
    #[tokio::test]
    async fn metrics_reflect_real_ratios() {
        let tracker = OutcomeTracker::new(10);

        // 7 successes + 3 failures = 70% accuracy/reliability.
        for i in 0..7 {
            tracker
                .record_with_duration(&format!("ok-{i}"), TaskOutcome::Success, 2_000.0)
                .await;
        }
        for i in 0..3 {
            tracker
                .record_with_duration(&format!("err-{i}"), TaskOutcome::Failure, 2_000.0)
                .await;
        }

        let metrics: std::collections::HashMap<_, _> =
            tracker.current_metrics().await.into_iter().collect();

        let accuracy = metrics[&PerfDimension::Accuracy];
        let reliability = metrics[&PerfDimension::Reliability];
        let speed = metrics[&PerfDimension::Speed];

        assert!((accuracy - 0.7).abs() < 1e-4, "expected ~0.70 accuracy, got {accuracy}");
        assert!((reliability - 0.7).abs() < 1e-4, "expected ~0.70 reliability, got {reliability}");
        // 2_000ms avg against 10_000ms baseline -> 0.8 Speed.
        assert!((speed - 0.8).abs() < 1e-4, "expected ~0.80 speed, got {speed}");
    }

    /// Empty tracker: all three metrics default to a perfect 1.0 so the
    /// evaluator does not raise spurious gaps before any data has flowed.
    #[tokio::test]
    async fn empty_tracker_reports_perfect_metrics() {
        let tracker = OutcomeTracker::new(5);
        let metrics: std::collections::HashMap<_, _> =
            tracker.current_metrics().await.into_iter().collect();

        for dim in [
            PerfDimension::Speed,
            PerfDimension::Accuracy,
            PerfDimension::Reliability,
        ] {
            assert!(
                (metrics[&dim] - 1.0).abs() < 1e-6,
                "empty tracker should report 1.0 for {dim:?}, got {}",
                metrics[&dim]
            );
        }
    }

    /// Speed clamps slow tasks at zero rather than going negative.
    #[tokio::test]
    async fn speed_clamps_for_extremely_slow_tasks() {
        let tracker = OutcomeTracker::new(5);
        tracker
            .record_with_duration("slow", TaskOutcome::Success, 50_000.0)
            .await;

        let metrics: std::collections::HashMap<_, _> =
            tracker.current_metrics().await.into_iter().collect();
        assert_eq!(metrics[&PerfDimension::Speed], 0.0);
    }

    /// The rolling window of recent outcomes never exceeds the 100-entry cap.
    #[tokio::test]
    async fn recent_window_caps_at_100_entries() {
        let tracker = OutcomeTracker::new(1_000);
        for i in 0..150 {
            tracker.record(&format!("t{i}"), TaskOutcome::Success).await;
        }
        let recent = tracker.recent().await;
        assert_eq!(recent.len(), 100);
        // Eldest retained entry corresponds to task index 50 (150 - 100).
        // We cannot assert task_id since it is not retained — we assert the
        // bound only.
        let state = tracker.state().await;
        assert_eq!(state.task_count, 150);
        assert_eq!(state.success_count, 150);
    }

    /// Operators can manually acknowledge the alert and the next failure
    /// run must start counting from zero again.
    #[tokio::test]
    async fn acknowledge_clears_alert_state() {
        let tracker = OutcomeTracker::new(2);
        tracker.record("a", TaskOutcome::Failure).await;
        let fired = tracker.record("b", TaskOutcome::Failure).await;
        assert!(fired);

        tracker.acknowledge_alert().await;
        let state = tracker.state().await;
        assert!(!state.alert_triggered);
        assert_eq!(state.consecutive_failures, 0);

        // A single failure post-ack must NOT re-trip the alert.
        let refired = tracker.record("c", TaskOutcome::Failure).await;
        assert!(!refired);
    }
}
