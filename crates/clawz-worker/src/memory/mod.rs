//! Memory subsystem for the ClawZ worker.
//!
//! This module provides persistent and ephemeral storage for agent memory,
//! including vector-backed semantic search, conversation history, and
//! cross-agent shared state.
//!
//! ## Sub-modules
//!
//! - [`store`]        — `PostgresMemoryBackend` (pgvector) and `InMemoryBackend`
//! - [`sqlite`]       — `SqliteMemoryBackend` (FTS5) for standalone/desktop
//! - [`compress`]     — Pre-LLM context compression for personal mode
//! - [`embedding`]    — Embedding providers (OpenAI, Ollama) and text chunker
//! - [`rag`]          — RAG pipeline (ingest + retrieve + rerank)
//! - [`conversation`] — Append-only conversation history with summarisation
//! - [`blackboard`]   — Multi-agent shared blackboard with pub/sub notifications
//!
//! ## Cross-module dependencies
//!
//! // Dependency: `clawz_core::traits::MemoryBackend` defines the contract
//! // implemented by both [`PostgresMemoryBackend`] and [`InMemoryBackend`].

pub mod archive;
pub mod behavioral_adaptor;
pub mod blackboard;
pub mod cache;
pub mod compress;
pub mod conversation;
pub mod embedding;
pub mod improvement;
pub mod outcome_tracker;
pub mod post_turn_nudge;
pub mod postgres_session_store;
pub mod rag;
pub mod rollup;
pub mod session_store;
pub mod sqlite;
pub mod store;
pub mod transcript_search;
pub mod tree;
pub mod user_profile;

// Re-export the most commonly used types at the module level.
pub use blackboard::{Blackboard, BlackboardEntry, ChangeEvent};
pub use compress::{CompressConfig, compress_messages};
pub use conversation::ConversationStore;
pub use embedding::{EmbeddingProvider, LocalEmbedding, OpenAIEmbedding, TextChunker};
pub use post_turn_nudge::PostTurnMemoryNudge;
pub use postgres_session_store::{PostgresSessionStore, open_session_store};
pub use rag::{RagConfig, RagPipeline};
pub use session_store::FileSessionStore;
pub use sqlite::{DEFAULT_MEMORY_DB, SqliteMemoryBackend, create_memory_backend};
pub use store::{InMemoryBackend, PostgresMemoryBackend};
pub use transcript_search::{TranscriptFtsIndex, TranscriptHit};
pub use tree::{MemoryTree, MemoryTreeNode};
pub use user_profile::{UserProfile, UserProfileStore};

// Re-export the core trait so callers do not need to reach into clawz_core.
pub use clawz_core::traits::MemoryBackend;
