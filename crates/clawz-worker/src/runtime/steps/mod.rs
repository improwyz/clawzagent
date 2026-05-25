//! Pipeline step registry — re-exports every concrete [`PipelineStep`] implementation.
//!
//! This module acts as a central catalogue so that [`AgentRuntime`](crate::runtime::agent::AgentRuntime)
//! (and any other pipeline builder) can import all steps from a single location.
//!
//! # Step inventory
//! | Step                 | File                | Purpose                                         |
//! |----------------------|---------------------|-------------------------------------------------|
//! | `ReceiveMessageStep` | `receive.rs`        | Ingest the incoming user message.               |
//! | `RetrieveContextStep`| `context.rs`        | RAG retrieval + system-prompt assembly.           |
//! | `SelectProviderStep` | `provider.rs`       | Route chat request to LLM, record cost.           |
//! | `ExecuteToolsStep`   | `tools.rs`          | Run tool calls returned by the model.             |
//! | `ApplyGovernanceStep`| `governance.rs`     | Policy check; may substitute safe decline.        |
//! | `PersistStateStep`   | `persist.rs`        | Save messages and agent state to memory backend.  |
//! | `StreamResponseStep` | `stream.rs`         | Yield final assistant response to caller.         |
//!
//! # Adding a new step
//! 1. Create `steps/<name>.rs` implementing [`PipelineStep`](clawz_core::traits::PipelineStep).
//! 2. Add `pub mod <name>;` below.
//! 3. Add `pub use <name>::<StepStruct>;` below.
//! 4. Wire the step into [`AgentRuntime::build_pipeline`](crate::runtime::agent::AgentRuntime::build_pipeline).

pub mod context;
pub mod governance;
pub mod persist;
pub mod provider;
pub mod receive;
pub mod stream;
pub mod tools;

/// Re-export: context-building step.
pub use context::RetrieveContextStep;
/// Re-export: governance policy evaluation step.
pub use governance::ApplyGovernanceStep;
/// Re-export: persistence step.
pub use persist::PersistStateStep;
/// Re-export: LLM provider selection and invocation step.
pub use provider::SelectProviderStep;
/// Re-export: incoming message ingestion step.
pub use receive::ReceiveMessageStep;
/// Re-export: response streaming / yielding step.
pub use stream::StreamResponseStep;
/// Re-export: tool execution step.
pub use tools::ExecuteToolsStep;
