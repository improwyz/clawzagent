//! REST API route handlers for the ClawZ Gateway.
//!
//! The gateway is the HTTP entry point for the platform. It receives external
//! requests, authenticates them, and schedules agents on fleet workers. This
//! module aggregates every sub-domain route handler into a single public API
//! surface so the main application can mount them under `/api/v1`.
//!
//! # Submodules
//!
//! | Module         | Domain                                         |
//! |----------------|------------------------------------------------|
//! | `agents`       | Agent lifecycle (CRUD, run, stop, history)     |
//! | `channels`     | Communication channel configuration            |
//! | `conversations`| Chat threads and message history             |
//! | `fleet`        | Worker-node fleet, mesh topology, deployments  |
//! | `governance`   | Policies, audit log, trust scoring             |
//! | `providers`    | External LLM / service provider configuration  |
//! | `system`       | Health, auth, metrics, config, OpenAPI spec    |
//! | `tools`        | Tool registry, execution, marketplace catalog  |

/// Agent lifecycle routes — creation, execution, stop, and history.
// Dependency: crate::routes::agents (reads/writes `AppState.agents`, `AppState.conversations`)
pub mod agents;

/// Channel configuration routes — manage inbound / outbound communication channels.
// Dependency: crate::routes::channels (reads/writes `AppState.channels`)
pub mod channels;

/// Conversation routes — threaded chat history and message send / receive.
// Dependency: crate::routes::conversations (reads/writes `AppState.conversations`, validates `AppState.agents`)
pub mod conversations;

/// Multi-participant agent room routes.
pub mod rooms;

/// Async room agent turn execution.
pub mod room_turn;

/// Bridge 1:1 conversations to multi-participant rooms.
pub mod conversation_room;

/// Fleet management routes — worker nodes, mesh topology, agent deployments.
// Dependency: crate::routes::fleet (reads/writes `AppState.fleet_nodes`, `AppState.deployments`, cross-checks `AppState.agents`)
pub mod fleet;

/// Governance routes — policy CRUD, audit log queries, trust scoring, evaluation.
// Dependency: crate::routes::governance (reads/writes `AppState.policies`, `AppState.audit_log`, reads `AppState.agents`)
pub mod governance;

/// Provider routes — external API endpoint registration (e.g. LLM backends).
// Dependency: crate::routes::providers (reads/writes `AppState.providers`)
pub mod providers;

/// System routes — health, metrics (Prometheus), auth, config, and OpenAPI spec.
// Dependency: crate::routes::system (reads `AppState.start_time`, `AppState.agents`, `AppState.fleet_nodes`, etc.)
pub mod system;

/// Dashboard JSON metrics for the web UI.
pub mod dashboard;

/// Cloud provider deployment API (`/cloud/*`).
pub mod cloud_deploy;

/// Tool registry routes — tool CRUD, execution, skills, plugins, marketplace.
// Dependency: crate::routes::tools (reads/writes `AppState.tools`)
pub mod tools;

/// Telephony webhooks (Twilio, Google Voice) — public, signature-verified.
pub mod telephony;
