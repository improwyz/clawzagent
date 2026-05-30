//! Background ingest and subconscious reflection ticks.

pub mod ingest;
pub mod subconscious;

pub use ingest::{ingest_memory_chunks, MemoryIngestChunk};
pub use subconscious::spawn_subconscious_scheduler;
