//! Background ingest and subconscious reflection ticks.

pub mod ingest;
pub mod subconscious;

pub use ingest::{MemoryIngestChunk, ingest_memory_chunks};
pub use subconscious::spawn_subconscious_scheduler;
