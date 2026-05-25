//! Workflow orchestration — DAG-based step execution with saga pattern.
//!
//! A `Workflow` is a collection of `WorkflowStep`s with declared dependencies.
//! Steps with no pending dependencies are executed in parallel; on failure,
//! compensating actions are run in reverse order (saga pattern).
//!
//! # Comparison with `Pipeline`
//! - `Pipeline` is a **linear** chain optimised for a single agent turn.
//! - `Workflow` is a **DAG** for multi-step business processes that may
//!   branch, merge, or run steps concurrently.
//!
//! # Saga pattern
//! Each [`WorkflowStepDef`] may declare an optional `compensate` handler.
//! If step `N` fails, the workflow invokes compensations for steps
//! `N-1 … 0` in reverse completion order.  This gives each completed step
//! a chance to undo its side effects (release reservations, refund credits,
//!   delete temporary files, etc.).
//!
//! # Cross-module relationships
//! - `Workflow` can embed `AgentRuntime::run` calls inside its step handlers
//!   to build higher-level agent workflows.
//! - `BranchCondition` provides simple if-then-else routing based on step output.
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

// Dependency: timestamp types for execution tracking.
use chrono::{DateTime, Utc};
// Dependency: core error types.
use clawz_core::error::{ClawzError, Result};
use serde::{Deserialize, Serialize};
// Dependency: async read-write lock for concurrent step output access.
use tokio::sync::RwLock;
use uuid::Uuid;

// ── WorkflowState machine ─────────────────────────────────────────────────────

/// Overall status of a workflow execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStatus {
    /// Created but not yet started.
    Pending,
    /// At least one step is currently running.
    Running,
    /// All steps finished successfully.
    Completed,
    /// A step failed and compensations ran.
    Failed,
    /// Explicitly cancelled by the caller.
    Cancelled,
}

impl std::fmt::Display for WorkflowStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkflowStatus::Pending => write!(f, "pending"),
            WorkflowStatus::Running => write!(f, "running"),
            WorkflowStatus::Completed => write!(f, "completed"),
            WorkflowStatus::Failed => write!(f, "failed"),
            WorkflowStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Status of an individual step within a workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    /// Waiting for dependencies.
    Pending,
    /// Currently executing.
    Running,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
    /// Skipped because a branch condition evaluated to false.
    Skipped,
}

// ── WorkflowOutput ─────────────────────────────────────────────────────────────

/// Key-value outputs produced by a workflow step.
///
/// Passed as input to dependent steps so they can read upstream results.
pub type StepOutput = HashMap<String, serde_json::Value>;

// ── WorkflowStepDef — user-defined step ───────────────────────────────────────

/// Handler function signature for a workflow step.
///
/// Receives the merged outputs of all dependency steps and returns its
/// own output map.
pub type StepHandler = Arc<
    dyn Fn(
            StepOutput, // inputs (outputs of dependency steps)
        ) -> Pin<Box<dyn std::future::Future<Output = Result<StepOutput>> + Send>>
        + Send
        + Sync,
>;

/// Compensating action (saga rollback).
///
/// Receives the step's own output so it knows what to undo.
pub type CompensateHandler = Arc<
    dyn Fn(
            StepOutput,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;

/// User-facing definition of a single workflow step.
///
/// `dependencies` lists the names of steps that must complete before this
/// one can run.  The workflow engine topologically sorts the steps and
/// gathers dependency outputs automatically.
pub struct WorkflowStepDef {
    /// Unique step name within the workflow.
    pub name: String,
    /// Names of steps that must finish before this step starts.
    pub dependencies: Vec<String>,
    /// Async handler that performs the step's work.
    pub handler: StepHandler,
    /// Optional rollback handler invoked during saga compensation.
    pub compensate: Option<CompensateHandler>,
}

impl WorkflowStepDef {
    /// Create a step without a compensation handler.
    pub fn new(
        name: impl Into<String>,
        dependencies: Vec<String>,
        handler: StepHandler,
    ) -> Self {
        Self {
            name: name.into(),
            dependencies,
            handler,
            compensate: None,
        }
    }

    /// Attach a compensating action for saga rollback.
    pub fn with_compensate(mut self, compensate: CompensateHandler) -> Self {
        self.compensate = Some(compensate);
        self
    }
}

// ── WorkflowExecution (internal state) ───────────────────────────────────────

/// Mutable per-step state tracked during workflow execution.
#[derive(Debug, Clone)]
struct StepState {
    status: StepStatus,
    output: Option<StepOutput>,
    error: Option<String>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

impl StepState {
    fn pending() -> Self {
        Self {
            status: StepStatus::Pending,
            output: None,
            error: None,
            started_at: None,
            completed_at: None,
        }
    }
}

// ── Workflow ─────────────────────────────────────────────────────────────────

/// A DAG-based workflow with saga-pattern rollback on failure.
///
/// Steps are stored in a `HashMap` keyed by name.  On first call to
/// [`Workflow::execute`], the engine computes a topological order via
/// Kahn's algorithm and then runs steps sequentially in that order.
///
/// # Concurrency note
/// Although the current implementation runs one step at a time, the
/// topological order already captures which steps *could* run in parallel.
/// A future enhancement can spawn independent branches concurrently.
pub struct Workflow {
    /// Unique workflow identifier (UUID v4).
    id: String,
    /// Human-readable workflow name.
    name: String,
    /// User-defined steps keyed by name.
    steps: HashMap<String, WorkflowStepDef>,
    /// Topological order (computed once on execute).
    step_order: Vec<String>,
}

impl Workflow {
    /// Create a new empty workflow.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            steps: HashMap::new(),
            step_order: Vec::new(),
        }
    }

    /// Return the workflow UUID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Return the workflow name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Register a step definition.
    pub fn add_step(&mut self, step: WorkflowStepDef) -> &mut Self {
        self.steps.insert(step.name.clone(), step);
        self
    }

    /// Topological sort using Kahn's algorithm.
    ///
    /// # Errors
    /// - [`ClawzError::Validation`] if a step references an unknown dependency.
    /// - [`ClawzError::Validation`] if the dependency graph contains a cycle.
    fn topological_sort(&self) -> Result<Vec<String>> {
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

        // Initialise in-degree map for every known step.
        for name in self.steps.keys() {
            in_degree.entry(name.as_str()).or_insert(0);
        }

        for (name, step) in &self.steps {
            for dep in &step.dependencies {
                if !self.steps.contains_key(dep.as_str()) {
                    return Err(ClawzError::Validation(format!(
                        "step '{}' depends on unknown step '{}'",
                        name, dep
                    )));
                }
                // Each dependency increases the in-degree of the dependent step.
                *in_degree.entry(name.as_str()).or_insert(0) += 1;
                dependents
                    .entry(dep.as_str())
                    .or_default()
                    .push(name.as_str());
            }
        }

        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter_map(|(&k, &v)| if v == 0 { Some(k) } else { None })
            .collect();

        let mut sorted = Vec::new();

        while !queue.is_empty() {
            queue.sort(); // deterministic ordering
            let node = queue.remove(0);
            sorted.push(node.to_string());

            if let Some(deps) = dependents.get(node) {
                for &dep in deps {
                    let deg = in_degree.get_mut(dep).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(dep);
                    }
                }
            }
        }

        // If sorted length != total steps, at least one node still has
        // in-degree > 0, meaning a cycle exists.
        if sorted.len() != self.steps.len() {
            return Err(ClawzError::Validation(
                "workflow has a dependency cycle".into(),
            ));
        }

        Ok(sorted)
    }

    /// Execute the workflow, returning the outputs of all steps.
    ///
    /// Steps whose dependencies are all satisfied run concurrently.
    /// On first step failure, run compensating actions in reverse and return error.
    pub async fn execute(&self) -> Result<HashMap<String, StepOutput>> {
        let order = self.topological_sort()?;
        // Shared, mutable output map.  Each completed step writes its result here.
        let outputs: Arc<RwLock<HashMap<String, StepOutput>>> =
            Arc::new(RwLock::new(HashMap::new()));
        // Stack of successfully completed step names for saga rollback.
        let completed: Arc<RwLock<Vec<String>>> = Arc::new(RwLock::new(Vec::new()));

        for step_name in &order {
            let step = self.steps.get(step_name).unwrap();

            // Gather dependency outputs.
            // We merge every dependency's output map into a single map so the
            // handler sees a flat namespace.  Name collisions are possible
            // but intentional: downstream steps can overwrite upstream keys.
            let dep_outputs = {
                let out = outputs.read().await;
                let mut merged = StepOutput::new();
                for dep in &step.dependencies {
                    if let Some(dep_out) = out.get(dep.as_str()) {
                        merged.extend(dep_out.clone());
                    }
                }
                merged
            };

            log::debug!("[workflow:{}] running step '{}'", self.name, step_name);

            match (step.handler)(dep_outputs.clone()).await {
                Ok(step_out) => {
                    outputs.write().await.insert(step_name.clone(), step_out);
                    completed.write().await.push(step_name.clone());
                }
                Err(e) => {
                    log::error!(
                        "[workflow:{}] step '{}' failed: {e}",
                        self.name,
                        step_name
                    );

                    // Saga rollback in reverse completed order.
                    // We clone the completed list so we can iterate while
                    // another task (not applicable in the sequential impl but
                    // kept for future parallel execution) might still touch
                    // the shared state.
                    let done = completed.read().await.clone();
                    for completed_name in done.iter().rev() {
                        if let Some(completed_step) = self.steps.get(completed_name) {
                            if let Some(compensate) = &completed_step.compensate {
                                let comp_in = outputs
                                    .read()
                                    .await
                                    .get(completed_name.as_str())
                                    .cloned()
                                    .unwrap_or_default();
                                log::debug!(
                                    "[workflow:{}] compensating '{}'",
                                    self.name,
                                    completed_name
                                );
                                if let Err(ce) = compensate(comp_in).await {
                                    log::error!(
                                        "[workflow:{}] compensation '{}' failed: {ce}",
                                        self.name,
                                        completed_name
                                    );
                                }
                            }
                        }
                    }

                    return Err(ClawzError::Internal(format!(
                        "workflow step '{}' failed: {e}",
                        step_name
                    )));
                }
            }
        }

        let final_outputs = outputs.read().await.clone();
        Ok(final_outputs)
    }
}

// ── ConditionalBranch ─────────────────────────────────────────────────────────

/// Simple conditional branching based on a step's output key.
///
/// Used to decide which branch of a workflow to follow after a step
/// finishes.  The condition is evaluated client-side (by the workflow
/// builder) rather than by the engine itself.
pub struct BranchCondition {
    /// Key in the [`StepOutput`] map to inspect.
    pub key: String,
    /// Value that selects the true branch.
    pub expected_value: serde_json::Value,
    /// Step name (or branch label) to follow when the key matches.
    pub true_branch: String,
    /// Step name (or branch label) to follow otherwise.
    pub false_branch: String,
}

impl BranchCondition {
    /// Create a new branch condition.
    pub fn new(
        key: impl Into<String>,
        expected_value: serde_json::Value,
        true_branch: impl Into<String>,
        false_branch: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            expected_value,
            true_branch: true_branch.into(),
            false_branch: false_branch.into(),
        }
    }

    /// Evaluate the condition against `outputs` and return the branch name.
    pub fn evaluate(&self, outputs: &StepOutput) -> &str {
        if outputs.get(&self.key) == Some(&self.expected_value) {
            &self.true_branch
        } else {
            &self.false_branch
        }
    }
}

// ── Builder helpers ───────────────────────────────────────────────────────────

/// Fluent builder for [`Workflow`].
///
/// Wraps the raw `Workflow` struct and provides an ergonomic API that
/// converts closures into the `Arc<dyn Fn…>` types expected by
/// [`WorkflowStepDef`].
pub struct WorkflowBuilder {
    workflow: Workflow,
}

impl WorkflowBuilder {
    /// Start building a workflow with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            workflow: Workflow::new(name),
        }
    }

    /// Add a step with its dependencies and handler.
    pub fn step<F, Fut>(
        mut self,
        name: impl Into<String>,
        deps: Vec<&str>,
        handler: F,
    ) -> Self
    where
        F: Fn(StepOutput) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<StepOutput>> + Send + 'static,
    {
        let handler: StepHandler = Arc::new(move |inp| Box::pin(handler(inp)));
        let step = WorkflowStepDef::new(
            name,
            deps.into_iter().map(|s| s.to_string()).collect(),
            handler,
        );
        self.workflow.add_step(step);
        self
    }

    /// Finalize and return the [`Workflow`].
    pub fn build(self) -> Workflow {
        self.workflow
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_simple_linear_workflow() {
        let wf = WorkflowBuilder::new("linear")
            .step("step1", vec![], |_| async {
                Ok([("x".to_string(), serde_json::json!(1))].into())
            })
            .step("step2", vec!["step1"], |inputs| async move {
                let x = inputs.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
                Ok([("y".to_string(), serde_json::json!(x + 1))].into())
            })
            .build();

        let outputs = wf.execute().await.unwrap();
        assert_eq!(outputs["step2"]["y"], serde_json::json!(2));
    }

    #[tokio::test]
    async fn test_workflow_missing_dep_fails() {
        let wf = WorkflowBuilder::new("bad")
            .step("step1", vec!["nonexistent"], |_| async {
                Ok(HashMap::new())
            })
            .build();

        assert!(wf.execute().await.is_err());
    }

    #[tokio::test]
    async fn test_saga_rollback_on_failure() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let compensated = Arc::new(AtomicBool::new(false));
        let compensated_clone = compensated.clone();

        let handler1: StepHandler = Arc::new(|_| {
            Box::pin(async { Ok(HashMap::new()) })
        });

        let comp: CompensateHandler = Arc::new(move |_| {
            let flag = compensated_clone.clone();
            Box::pin(async move {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            })
        });

        let step1 = WorkflowStepDef::new("step1", vec![], handler1)
            .with_compensate(comp);

        let handler2: StepHandler = Arc::new(|_| {
            Box::pin(async {
                Err(ClawzError::Internal("intentional".into()))
            })
        });
        let step2 = WorkflowStepDef::new("step2", vec!["step1".to_string()], handler2);

        let mut wf = Workflow::new("saga_test");
        wf.add_step(step1);
        wf.add_step(step2);

        assert!(wf.execute().await.is_err());
        assert!(compensated.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_branch_condition() {
        let out: StepOutput = [("flag".to_string(), serde_json::json!(true))].into();
        let branch = BranchCondition::new(
            "flag",
            serde_json::json!(true),
            "branch_true",
            "branch_false",
        );
        assert_eq!(branch.evaluate(&out), "branch_true");

        let out2: StepOutput = [("flag".to_string(), serde_json::json!(false))].into();
        assert_eq!(branch.evaluate(&out2), "branch_false");
    }

    #[tokio::test]
    async fn test_topological_cycle_detected() {
        let h: StepHandler =
            Arc::new(|_| Box::pin(async { Ok(HashMap::new()) }));

        let mut wf = Workflow::new("cycle");
        wf.add_step(WorkflowStepDef::new(
            "a",
            vec!["b".to_string()],
            h.clone(),
        ));
        wf.add_step(WorkflowStepDef::new(
            "b",
            vec!["a".to_string()],
            h,
        ));

        assert!(wf.execute().await.is_err());
    }
}
