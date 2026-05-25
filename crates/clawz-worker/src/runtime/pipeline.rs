//! Pipeline execution engine.
//!
//! Executes an ordered sequence of [`PipelineStep`]s, tracks per-step duration,
//! supports conditional steps (skip if condition not met), and rolls back
//! completed steps in reverse order on failure.
//!
//! # Design rationale
//! The pipeline is intentionally simple: it is *not* a DAG executor (see
//! [`orchestration::Workflow`](crate::runtime::orchestration::Workflow) for that).
//! It is a linear chain because an agent turn is fundamentally sequential:
//! receive → think → act → check → save → respond.
//!
//! # Rollback semantics
//! If step `N` fails, steps `N-1 … 0` are rolled back in reverse order via
//! their [`PipelineStep::rollback`] implementation.  This gives each step a
//! chance to undo side effects (delete saved messages, release locks, etc.).
//!
//! # Dependencies
//! - `clawz_core::traits` — [`PipelineStep`], [`PipelineContext`], [`StepOutcome`]

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
// Dependency: core error types and pipeline abstractions from `clawz_core`.
use clawz_core::{
    error::Result,
    traits::{PipelineContext, PipelineStep, StepOutcome},
};

/// A named condition evaluated before running a step.
/// Returns `true` if the step should run, `false` to skip.
///
/// Conditions are evaluated synchronously so the pipeline can decide
/// whether to spawn the async step without awaiting anything.
pub type StepCondition = Arc<dyn Fn(&PipelineContext) -> bool + Send + Sync>;

/// Internal registration entry for a step in the pipeline.
///
/// Wraps the step trait object together with its optional run-time condition.
struct StepEntry {
    /// The concrete step implementation.
    step: Arc<dyn PipelineStep>,
    /// If `Some`, the step is only executed when the condition returns `true`.
    condition: Option<StepCondition>,
}

/// Per-step timing metric captured during pipeline execution.
#[derive(Debug, Clone)]
pub struct StepMetric {
    /// Human-readable step name (from [`PipelineStep::name`]).
    pub step_name: String,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u128,
    /// High-level outcome: `"continue"`, `"halt"`, `"skipped"`, or `"error: …"`.
    pub outcome: String,
}

/// Result of a full pipeline execution.
#[derive(Debug)]
pub struct PipelineResult {
    /// Final outcome after the last executed step.
    pub outcome: StepOutcome,
    /// Timing metrics for every step that was reached (including skipped ones).
    pub metrics: Vec<StepMetric>,
    /// Name of the step that caused a halt, if any.
    pub halted_at: Option<String>,
}

/// Ordered pipeline of [`PipelineStep`]s.
///
/// Steps are registered by name and executed in registration order.  On any
/// `Err`, all already-completed steps are rolled back in reverse order.
///
/// # Usage
/// Prefer [`PipelineBuilder`] for fluent construction:
/// ```ignore
/// let pipeline = PipelineBuilder::new("my_pipeline")
///     .step(MyStep)
///     .conditional_step(ExtraStep, |ctx| ctx.has_flag("extra"))
///     .build();
/// ```
pub struct Pipeline {
    /// Human-readable pipeline name used in logs and metrics.
    name: String,
    /// Ordered list of registered steps.
    steps: Vec<StepEntry>,
}

impl Pipeline {
    /// Create a new empty pipeline with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            steps: Vec::new(),
        }
    }

    /// Append a step (always executed when reached).
    pub fn add_step(&mut self, step: Arc<dyn PipelineStep>) -> &mut Self {
        self.steps.push(StepEntry {
            step,
            condition: None,
        });
        self
    }

    /// Append a step with a run-time condition.  The step is skipped if the
    /// condition returns `false`.
    ///
    /// This is useful for optional steps such as "run tool execution only
    /// when the last assistant message contained tool calls".
    pub fn add_conditional_step(
        &mut self,
        step: Arc<dyn PipelineStep>,
        condition: StepCondition,
    ) -> &mut Self {
        self.steps.push(StepEntry {
            step,
            condition: Some(condition),
        });
        self
    }

    /// Return the pipeline name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Execute all steps in order, rolling back on failure.
    ///
    /// # Algorithm
    /// 1. For each step, evaluate its optional condition.
    /// 2. If the condition passes (or is absent), call `step.execute(ctx).await`.
    /// 3. On `Continue`, record the metric and push the step index onto the
    ///    `completed` stack.
    /// 4. On `Halt` or `Delegate`, return immediately with the outcome.
    /// 5. On `Err`, roll back every step in `completed` (reverse order),
    ///    then return the error.
    pub async fn execute(&self, ctx: &mut PipelineContext) -> Result<PipelineResult> {
        let mut metrics: Vec<StepMetric> = Vec::with_capacity(self.steps.len());
        // Stack of step indices that completed successfully; used for rollback.
        let mut completed: Vec<usize> = Vec::new();

        for (idx, entry) in self.steps.iter().enumerate() {
            // Evaluate optional condition.
            if let Some(cond) = &entry.condition {
                if !cond(ctx) {
                    log::debug!(
                        "[pipeline:{}] skipping step '{}' (condition false)",
                        self.name,
                        entry.step.name()
                    );
                    metrics.push(StepMetric {
                        step_name: entry.step.name().to_string(),
                        duration_ms: 0,
                        outcome: "skipped".to_string(),
                    });
                    continue;
                }
            }

            let step_name = entry.step.name().to_string();
            log::debug!(
                "[pipeline:{}] executing step '{}'",
                self.name,
                step_name
            );

            let start = Instant::now();
            match entry.step.execute(ctx).await {
                Ok(StepOutcome::Continue) => {
                    let elapsed = start.elapsed().as_millis();
                    metrics.push(StepMetric {
                        step_name: step_name.clone(),
                        duration_ms: elapsed,
                        outcome: "continue".to_string(),
                    });
                    completed.push(idx);
                }
                Ok(StepOutcome::Halt) => {
                    let elapsed = start.elapsed().as_millis();
                    metrics.push(StepMetric {
                        step_name: step_name.clone(),
                        duration_ms: elapsed,
                        outcome: "halt".to_string(),
                    });
                    log::info!("[pipeline:{}] halted at step '{}'", self.name, step_name);
                    return Ok(PipelineResult {
                        outcome: StepOutcome::Halt,
                        metrics,
                        halted_at: Some(step_name),
                    });
                }
                Ok(StepOutcome::Delegate { target_agent }) => {
                    let elapsed = start.elapsed().as_millis();
                    metrics.push(StepMetric {
                        step_name: step_name.clone(),
                        duration_ms: elapsed,
                        outcome: format!("delegate:{target_agent}"),
                    });
                    log::info!(
                        "[pipeline:{}] delegating to '{target_agent}' at step '{}'",
                        self.name,
                        step_name
                    );
                    return Ok(PipelineResult {
                        outcome: StepOutcome::Delegate { target_agent },
                        metrics,
                        halted_at: Some(step_name),
                    });
                }
                Err(e) => {
                    let elapsed = start.elapsed().as_millis();
                    metrics.push(StepMetric {
                        step_name: step_name.clone(),
                        duration_ms: elapsed,
                        outcome: format!("error: {e}"),
                    });
                    log::error!(
                        "[pipeline:{}] step '{}' failed: {e}",
                        self.name,
                        step_name
                    );

                    // Roll back completed steps in reverse order.
                    // This guarantees that if step 2 fails, step 1's rollback
                    // runs before step 0's, preserving nested side-effect order.
                    for &prev_idx in completed.iter().rev() {
                        let prev_name = self.steps[prev_idx].step.name();
                        log::debug!(
                            "[pipeline:{}] rolling back step '{}'",
                            self.name,
                            prev_name
                        );
                        if let Err(rb_err) =
                            self.steps[prev_idx].step.rollback(ctx).await
                        {
                            log::error!(
                                "[pipeline:{}] rollback of '{}' failed: {rb_err}",
                                self.name,
                                prev_name
                            );
                        }
                    }

                    return Err(e);
                }
            }
        }

        Ok(PipelineResult {
            outcome: StepOutcome::Continue,
            metrics,
            halted_at: None,
        })
    }
}

// ── Builder helper ─────────────────────────────────────────────────────────────

/// Fluent builder for [`Pipeline`].
///
/// Consumes `self` on every call so chains are ergonomic and type-safe.
pub struct PipelineBuilder {
    inner: Pipeline,
}

impl PipelineBuilder {
    /// Start building a pipeline with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            inner: Pipeline::new(name),
        }
    }

    /// Append an unconditional step.
    pub fn step(mut self, step: impl PipelineStep + 'static) -> Self {
        self.inner.add_step(Arc::new(step));
        self
    }

    /// Append a conditional step.
    pub fn conditional_step(
        mut self,
        step: impl PipelineStep + 'static,
        condition: impl Fn(&PipelineContext) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.inner
            .add_conditional_step(Arc::new(step), Arc::new(condition));
        self
    }

    /// Finalize and return the [`Pipeline`].
    pub fn build(self) -> Pipeline {
        self.inner
    }
}

// ── No-op step for testing ─────────────────────────────────────────────────────

/// A no-op step that always returns `Continue`.
///
/// Useful in unit tests that need to verify pipeline mechanics
/// (ordering, rollback, halting) without real step logic.
pub struct NoopStep {
    name: String,
}

impl NoopStep {
    /// Create a no-op step with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl PipelineStep for NoopStep {
    fn name(&self) -> &str {
        &self.name
    }

    async fn execute(&self, _ctx: &mut PipelineContext) -> Result<StepOutcome> {
        Ok(StepOutcome::Continue)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::traits::PipelineContext;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_ctx() -> PipelineContext {
        PipelineContext::new("agent-1", "conv-1")
    }

    #[tokio::test]
    async fn test_empty_pipeline_continues() {
        let p = Pipeline::new("test");
        let mut ctx = make_ctx();
        let res = p.execute(&mut ctx).await.unwrap();
        assert!(matches!(res.outcome, StepOutcome::Continue));
    }

    #[tokio::test]
    async fn test_pipeline_runs_all_steps() {
        let counter = Arc::new(AtomicUsize::new(0));

        struct CountStep {
            name: String,
            counter: Arc<AtomicUsize>,
        }
        #[async_trait]
        impl PipelineStep for CountStep {
            fn name(&self) -> &str {
                &self.name
            }
            async fn execute(
                &self,
                _ctx: &mut PipelineContext,
            ) -> Result<StepOutcome> {
                self.counter.fetch_add(1, Ordering::SeqCst);
                Ok(StepOutcome::Continue)
            }
        }

        let mut p = Pipeline::new("test");
        p.add_step(Arc::new(CountStep {
            name: "s1".into(),
            counter: counter.clone(),
        }));
        p.add_step(Arc::new(CountStep {
            name: "s2".into(),
            counter: counter.clone(),
        }));

        let mut ctx = make_ctx();
        p.execute(&mut ctx).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_halt_stops_pipeline() {
        struct HaltStep;
        #[async_trait]
        impl PipelineStep for HaltStep {
            fn name(&self) -> &str {
                "halt"
            }
            async fn execute(
                &self,
                _ctx: &mut PipelineContext,
            ) -> Result<StepOutcome> {
                Ok(StepOutcome::Halt)
            }
        }

        let counter = Arc::new(AtomicUsize::new(0));
        struct CountStep2(Arc<AtomicUsize>);
        #[async_trait]
        impl PipelineStep for CountStep2 {
            fn name(&self) -> &str {
                "count"
            }
            async fn execute(
                &self,
                _ctx: &mut PipelineContext,
            ) -> Result<StepOutcome> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(StepOutcome::Continue)
            }
        }

        let p = PipelineBuilder::new("test")
            .step(HaltStep)
            .step(CountStep2(counter.clone()))
            .build();

        let mut ctx = make_ctx();
        let res = p.execute(&mut ctx).await.unwrap();
        assert!(matches!(res.outcome, StepOutcome::Halt));
        // Step after halt should not run.
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_error_triggers_rollback() {
        use std::sync::Mutex;
        let rolled_back = Arc::new(Mutex::new(Vec::<String>::new()));

        struct TrackStep {
            name: String,
            rolled_back: Arc<Mutex<Vec<String>>>,
        }
        #[async_trait]
        impl PipelineStep for TrackStep {
            fn name(&self) -> &str {
                &self.name
            }
            async fn execute(
                &self,
                _ctx: &mut PipelineContext,
            ) -> Result<StepOutcome> {
                Ok(StepOutcome::Continue)
            }
            async fn rollback(&self, _ctx: &mut PipelineContext) -> Result<()> {
                self.rolled_back.lock().unwrap().push(self.name.clone());
                Ok(())
            }
        }

        struct FailStep;
        #[async_trait]
        impl PipelineStep for FailStep {
            fn name(&self) -> &str {
                "fail"
            }
            async fn execute(
                &self,
                _ctx: &mut PipelineContext,
            ) -> Result<StepOutcome> {
                Err(ClawzError::Internal("intentional failure".into()))
            }
        }

        let p = PipelineBuilder::new("test")
            .step(TrackStep {
                name: "s1".into(),
                rolled_back: rolled_back.clone(),
            })
            .step(TrackStep {
                name: "s2".into(),
                rolled_back: rolled_back.clone(),
            })
            .step(FailStep)
            .build();

        let mut ctx = make_ctx();
        assert!(p.execute(&mut ctx).await.is_err());
        let rb = rolled_back.lock().unwrap();
        // Rolled back in reverse: s2, s1
        assert_eq!(rb.as_slice(), ["s2", "s1"]);
    }

    #[tokio::test]
    async fn test_conditional_step_skipped() {
        let counter = Arc::new(AtomicUsize::new(0));
        struct CountStep3(Arc<AtomicUsize>);
        #[async_trait]
        impl PipelineStep for CountStep3 {
            fn name(&self) -> &str {
                "cond"
            }
            async fn execute(
                &self,
                _ctx: &mut PipelineContext,
            ) -> Result<StepOutcome> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(StepOutcome::Continue)
            }
        }

        let p = PipelineBuilder::new("test")
            .conditional_step(CountStep3(counter.clone()), |_ctx| false)
            .build();

        let mut ctx = make_ctx();
        p.execute(&mut ctx).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
}
