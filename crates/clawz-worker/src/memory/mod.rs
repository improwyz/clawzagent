//! Memory subsystem for the ClawZ worker.
//!
//! This module provides persistent and ephemeral storage for agent memory,
//! including vector-backed semantic search, conversation history, and
//! cross-agent shared state.
//!
//! ## Sub-modules
//!
//! - [`store`]        — `PostgresMemoryBackend` (pgvector) and `InMemoryBackend`
//! - [`embedding`]    — Embedding providers (OpenAI, Ollama) and text chunker
//! - [`rag`]          — RAG pipeline (ingest + retrieve + rerank)
//! - [`conversation`] — Append-only conversation history with summarisation
//! - [`blackboard`]   — Multi-agent shared blackboard with pub/sub notifications
//!
//! ## Cross-module dependencies
//!
//! // Dependency: `clawz_core::traits::MemoryBackend` defines the contract
//! // implemented by both [`PostgresMemoryBackend`] and [`InMemoryBackend`].

pub mod blackboard;
pub mod conversation;
pub mod embedding;
pub mod rag;
pub mod archive;
pub mod cache;
pub mod improvement;
pub mod outcome_tracker;
pub mod store;

// Re-export the most commonly used types at the module level.
pub use blackboard::{Blackboard, BlackboardEntry, ChangeEvent};
pub use conversation::ConversationStore;
pub use embedding::{EmbeddingProvider, LocalEmbedding, OpenAIEmbedding, TextChunker};
pub use rag::{RagConfig, RagPipeline};
pub use store::{InMemoryBackend, PostgresMemoryBackend};

// Re-export the core trait so callers do not need to reach into clawz_core.
pub use clawz_core::traits::MemoryBackend;
