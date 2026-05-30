//! Hierarchical memory tree for standalone / desktop mode.
//!
//! Nodes form a parent/child graph per agent. Rollups and post-turn hooks append
//! summaries; [`RetrieveContextStep`] injects matching branches into the system prompt.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use tokio::task;
use uuid::Uuid;

use clawz_core::deployment::DeploymentMode;
use clawz_core::error::{ClawzError, Result};

/// One node in the agent memory tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryTreeNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub agent_id: String,
    pub title: String,
    pub summary: String,
    pub depth: u32,
    pub created_at: String,
}

/// SQLite-backed hierarchical memory graph (shares `memory.db` with [`super::sqlite::SqliteMemoryBackend`]).
#[derive(Debug, Clone)]
pub struct MemoryTree {
    db_path: PathBuf,
}

static GLOBAL_TREE: OnceCell<Arc<MemoryTree>> = OnceCell::const_new();

impl MemoryTree {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db_path = path.as_ref().to_path_buf();
        let tree = Self { db_path };
        tree.migrate().await?;
        Ok(tree)
    }

    pub async fn open_default() -> Result<Self> {
        Self::open(super::sqlite::SqliteMemoryBackend::default_db_path()).await
    }

    /// Shared tree instance for pipeline steps (standalone only).
    pub async fn global() -> Result<Arc<Self>> {
        if DeploymentMode::from_env() != DeploymentMode::Standalone {
            return Err(ClawzError::Internal(
                "memory tree is standalone-only".into(),
            ));
        }
        GLOBAL_TREE
            .get_or_try_init(|| async {
                Ok(Arc::new(Self::open_default().await?))
            })
            .await
            .map(Arc::clone)
    }

    async fn migrate(&self) -> Result<()> {
        let path = self.db_path.clone();
        task::spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ClawzError::Database(format!("memory tree dir: {e}"))
                })?;
            }
            let conn = Connection::open(&path)
                .map_err(|e| ClawzError::Database(format!("memory tree open: {e}")))?;
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS clawz_memory_tree (
                    id          TEXT PRIMARY KEY,
                    parent_id   TEXT,
                    agent_id    TEXT NOT NULL,
                    title       TEXT NOT NULL,
                    summary     TEXT NOT NULL,
                    depth       INTEGER NOT NULL DEFAULT 0,
                    created_at  TEXT NOT NULL,
                    FOREIGN KEY (parent_id) REFERENCES clawz_memory_tree(id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS clawz_memory_tree_agent_idx
                    ON clawz_memory_tree (agent_id, depth, created_at DESC);
                "#,
            )
            .map_err(|e| ClawzError::Database(format!("memory tree migrate: {e}")))?;
            Ok::<(), ClawzError>(())
        })
        .await
        .map_err(|e| ClawzError::Internal(format!("memory tree migrate join: {e}")))??;
        Ok(())
    }

    /// Insert a node; returns the new id.
    pub async fn insert(
        &self,
        agent_id: &str,
        parent_id: Option<&str>,
        title: impl Into<String>,
        summary: impl Into<String>,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let depth = if let Some(pid) = parent_id {
            self.depth_of(pid).await? + 1
        } else {
            0
        };
        let node = MemoryTreeNode {
            id: id.clone(),
            parent_id: parent_id.map(str::to_string),
            agent_id: agent_id.to_string(),
            title: title.into(),
            summary: summary.into(),
            depth,
            created_at: Utc::now().to_rfc3339(),
        };
        self.persist_node(&node).await?;
        Ok(id)
    }

    async fn depth_of(&self, node_id: &str) -> Result<u32> {
        let path = self.db_path.clone();
        let node_id = node_id.to_string();
        task::spawn_blocking(move || {
            let conn = Connection::open(&path)
                .map_err(|e| ClawzError::Database(format!("memory tree open: {e}")))?;
            let depth: i64 = conn
                .query_row(
                    "SELECT depth FROM clawz_memory_tree WHERE id = ?1",
                    params![node_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok::<u32, ClawzError>(depth as u32)
        })
        .await
        .map_err(|e| ClawzError::Internal(format!("memory tree depth join: {e}")))?
    }

    async fn persist_node(&self, node: &MemoryTreeNode) -> Result<()> {
        let node = node.clone();
        let path = self.db_path.clone();
        task::spawn_blocking(move || {
            let conn = Connection::open(&path)
                .map_err(|e| ClawzError::Database(format!("memory tree open: {e}")))?;
            conn.execute(
                r#"
                INSERT INTO clawz_memory_tree (id, parent_id, agent_id, title, summary, depth, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
                params![
                    node.id,
                    node.parent_id,
                    node.agent_id,
                    node.title,
                    node.summary,
                    node.depth,
                    node.created_at,
                ],
            )
            .map_err(|e| ClawzError::Database(format!("memory tree insert: {e}")))?;
            Ok::<(), ClawzError>(())
        })
        .await
        .map_err(|e| ClawzError::Internal(format!("memory tree insert join: {e}")))??;
        Ok(())
    }

    /// All nodes for an agent, roots first.
    pub async fn list_agent(&self, agent_id: &str) -> Result<Vec<MemoryTreeNode>> {
        let agent_id = agent_id.to_string();
        let path = self.db_path.clone();
        task::spawn_blocking(move || {
            let conn = Connection::open(&path)
                .map_err(|e| ClawzError::Database(format!("memory tree open: {e}")))?;
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, parent_id, agent_id, title, summary, depth, created_at
                    FROM clawz_memory_tree
                    WHERE agent_id = ?1
                    ORDER BY depth ASC, created_at DESC
                    "#,
                )
                .map_err(|e| ClawzError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![agent_id], |row| {
                    Ok(MemoryTreeNode {
                        id: row.get(0)?,
                        parent_id: row.get(1)?,
                        agent_id: row.get(2)?,
                        title: row.get(3)?,
                        summary: row.get(4)?,
                        depth: row.get::<_, i64>(5)? as u32,
                        created_at: row.get(6)?,
                    })
                })
                .map_err(|e| ClawzError::Database(e.to_string()))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| ClawzError::Database(e.to_string()))?);
            }
            Ok::<Vec<MemoryTreeNode>, ClawzError>(out)
        })
        .await
        .map_err(|e| ClawzError::Internal(format!("memory tree list join: {e}")))?
    }

    /// Keyword match over title + summary for context injection.
    pub async fn search(&self, agent_id: &str, query: &str, limit: usize) -> Result<Vec<MemoryTreeNode>> {
        let q = query.to_lowercase();
        let mut nodes = self.list_agent(agent_id).await?;
        if q.is_empty() {
            nodes.truncate(limit);
            return Ok(nodes);
        }
        nodes.retain(|n| {
            n.title.to_lowercase().contains(&q) || n.summary.to_lowercase().contains(&q)
        });
        nodes.truncate(limit);
        Ok(nodes)
    }

    /// Render a markdown block for the LLM system prompt.
    pub fn format_context(nodes: &[MemoryTreeNode]) -> String {
        if nodes.is_empty() {
            return String::new();
        }
        let mut block = String::from("\n\n## Memory Tree\n");
        for n in nodes {
            block.push_str(&format!(
                "- **{}** (depth {}): {}\n",
                n.title, n.depth, n.summary
            ));
        }
        block
    }

    /// Append a rollup summary node (hourly / post-turn).
    pub async fn append_rollup(&self, agent_id: &str, summary: impl Into<String>) -> Result<()> {
        let title = format!("rollup-{}", Utc::now().format("%Y-%m-%d-%H"));
        self.insert(agent_id, None, title, summary).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_tree_insert_and_search() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("tree.db");
        let tree = MemoryTree::open(&db).await.unwrap();
        tree.insert("agent-1", None, "root", "project context")
            .await
            .unwrap();
        tree.insert("agent-1", None, "session", "discussed deployment")
            .await
            .unwrap();
        let hits = tree.search("agent-1", "deployment", 5).await.unwrap();
        assert!(!hits.is_empty());
        let ctx = MemoryTree::format_context(&hits);
        assert!(ctx.contains("Memory Tree"));
    }
}
