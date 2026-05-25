use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub key: String,
    pub value: String,
    pub timestamp: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait ArchiveBackend: Send + Sync {
    async fn archive(&self, entries: Vec<MemoryEntry>) -> clawz_core::Result<()>;
    async fn retrieve(&self, query: &str) -> clawz_core::Result<Vec<MemoryEntry>>;
}

pub struct FilesystemArchive {
    base_dir: PathBuf,
}

impl FilesystemArchive {
    pub fn new(base_dir: PathBuf) -> Self { Self { base_dir } }
}

#[async_trait::async_trait]
impl ArchiveBackend for FilesystemArchive {
    async fn archive(&self, entries: Vec<MemoryEntry>) -> clawz_core::Result<()> {
        std::fs::create_dir_all(&self.base_dir)
            .map_err(|e| clawz_core::error::ClawzError::Internal(format!("mkdir: {e}")))?;
        let path = self.base_dir.join("archive.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .create(true).append(true).open(&path)
            .map_err(|e| clawz_core::error::ClawzError::Internal(format!("open: {e}")))?;
        for entry in &entries {
            let line = serde_json::to_string(entry)
                .map_err(|e| clawz_core::error::ClawzError::Internal(format!("json: {e}")))?;
            use std::io::Write;
            writeln!(file, "{}", line)
                .map_err(|e| clawz_core::error::ClawzError::Internal(format!("write: {e}")))?;
        }
        Ok(())
    }

    async fn retrieve(&self, query: &str) -> clawz_core::Result<Vec<MemoryEntry>> {
        let path = self.base_dir.join("archive.jsonl");
        if !path.exists() { return Ok(vec![]); }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| clawz_core::error::ClawzError::Internal(format!("read: {e}")))?;
        let mut results = Vec::new();
        for line in content.lines() {
            if let Ok(entry) = serde_json::from_str::<MemoryEntry>(line) {
                if entry.key.contains(query) || entry.value.contains(query) {
                    results.push(entry);
                }
            }
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn filesystem_archive_roundtrip() {
        let dir = std::env::temp_dir().join(format!("clawz_archive_{}", uuid::Uuid::new_v4()));
        let archive = FilesystemArchive::new(dir.clone());
        let entries = vec![
            MemoryEntry { key: "test_key".into(), value: "test_value".into(), timestamp: Utc::now() },
        ];
        archive.archive(entries).await.unwrap();
        let results = archive.retrieve("test_key").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "test_key");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
