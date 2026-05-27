use crate::tools::tool_trait::{Tool, ToolContext};
use async_trait::async_trait;
use clawz_core::error::ClawzError;
use clawz_core::types::tool_risk::{ActionPrimitive, RiskLevel};
use clawz_core::types::{ToolResult, ToolSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Knowledge base tool that queries/stores information via the RAG pipeline.
/// Uses PostgreSQL + pgvector when the CLAWZ_DB_URL is configured.
pub struct KnowledgeBaseTool;

impl KnowledgeBaseTool {
    pub fn new() -> Self {
        Self
    }

    async fn query_rag(
        query: &str,
        top_k: usize,
        agent_id: &str,
        namespace: Option<&str>,
    ) -> Result<Vec<KbEntry>, ClawzError> {
        // Connect to the pgvector-backed knowledge base
        let db_url = std::env::var("CLAWZ_DB_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .map_err(|_| {
                ClawzError::Config("CLAWZ_DB_URL not set — knowledge base unavailable".into())
            })?;

        // Build query embedding via the embedding API
        let embedding = get_embedding(query).await?;
        let ns = namespace.unwrap_or("default");

        let pool = sqlx::PgPool::connect(&db_url)
            .await
            .map_err(|e| ClawzError::Database(format!("DB connect failed: {e}")))?;

        // Format embedding as pgvector literal
        let embedding_str = format!(
            "[{}]",
            embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let sql = format!(
            r#"
            SELECT id::text, content, metadata::text,
                   1 - (embedding <=> '{}'::vector) AS similarity
            FROM knowledge_entries
            WHERE (agent_id = $1 OR agent_id IS NULL)
              AND namespace = $2
            ORDER BY embedding <=> '{}'::vector
            LIMIT $3
            "#,
            embedding_str, embedding_str
        );

        let rows = sqlx::query_as::<_, (String, String, Option<String>, f64)>(&sql)
            .bind(agent_id)
            .bind(ns)
            .bind(top_k as i64)
            .fetch_all(&pool)
            .await
            .map_err(|e| ClawzError::Database(format!("vector search failed: {e}")))?;

        let entries = rows
            .into_iter()
            .map(|(id, content, metadata_str, similarity)| KbEntry {
                id,
                content,
                metadata: metadata_str
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or(Value::Null),
                similarity,
            })
            .collect();

        pool.close().await;
        Ok(entries)
    }

    async fn store_entry(
        content: &str,
        metadata: Value,
        agent_id: &str,
        namespace: Option<&str>,
    ) -> Result<String, ClawzError> {
        let db_url = std::env::var("CLAWZ_DB_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .map_err(|_| {
                ClawzError::Config("CLAWZ_DB_URL not set — knowledge base unavailable".into())
            })?;

        let embedding = get_embedding(content).await?;
        let ns = namespace.unwrap_or("default");

        let pool = sqlx::PgPool::connect(&db_url)
            .await
            .map_err(|e| ClawzError::Database(format!("DB connect failed: {e}")))?;

        let embedding_str = format!(
            "[{}]",
            embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let metadata_json = serde_json::to_string(&metadata).unwrap_or_else(|_| "null".into());

        let sql = format!(
            r#"
            INSERT INTO knowledge_entries (content, embedding, metadata, agent_id, namespace, created_at)
            VALUES ($1, '{}'::vector, $2::jsonb, $3, $4, NOW())
            ON CONFLICT DO NOTHING
            RETURNING id::text
            "#,
            embedding_str
        );

        let row: Option<(String,)> = sqlx::query_as::<_, (String,)>(&sql)
            .bind(content)
            .bind(metadata_json)
            .bind(agent_id)
            .bind(ns)
            .fetch_optional(&pool)
            .await
            .map_err(|e| ClawzError::Database(format!("insert failed: {e}")))?;

        pool.close().await;
        Ok(row.map(|(id,)| id).unwrap_or_else(|| "duplicate".into()))
    }

    async fn delete_entry(id: &str, agent_id: &str) -> Result<bool, ClawzError> {
        let db_url = std::env::var("CLAWZ_DB_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .map_err(|_| ClawzError::Config("CLAWZ_DB_URL not set".into()))?;

        let pool = sqlx::PgPool::connect(&db_url)
            .await
            .map_err(|e| ClawzError::Database(format!("DB connect: {e}")))?;

        let result: Option<(String,)> = sqlx::query_as::<_, (String,)>(
            "DELETE FROM knowledge_entries WHERE id = $1::uuid AND agent_id = $2 RETURNING id::text"
        )
        .bind(id)
        .bind(agent_id)
        .fetch_optional(&pool)
        .await
        .map_err(|e| ClawzError::Database(format!("delete failed: {e}")))?;

        pool.close().await;
        Ok(result.is_some())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct KbEntry {
    pub id: String,
    pub content: String,
    pub metadata: Value,
    pub similarity: f64,
}

/// Get text embedding from configured embedding model.
async fn get_embedding(text: &str) -> Result<Vec<f32>, ClawzError> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("EMBEDDING_API_KEY"))
        .map_err(|_| ClawzError::Config("OPENAI_API_KEY not set for embeddings".into()))?;

    let model =
        std::env::var("EMBEDDING_MODEL").unwrap_or_else(|_| "text-embedding-3-small".into());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ClawzError::Tool(format!("HTTP client failed: {e}")))?;

    let body = serde_json::json!({
        "model": model,
        "input": text
    });

    let resp: Value = client
        .post("https://api.openai.com/v1/embeddings")
        .bearer_auth(&api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| ClawzError::Tool(format!("embedding API request failed: {e}")))?
        .json()
        .await
        .map_err(|e| ClawzError::Tool(format!("embedding response parse failed: {e}")))?;

    let embedding: Vec<f32> = resp["data"][0]["embedding"]
        .as_array()
        .ok_or_else(|| ClawzError::Tool("no embedding in response".into()))?
        .iter()
        .filter_map(|v| v.as_f64().map(|f| f as f32))
        .collect();

    if embedding.is_empty() {
        return Err(ClawzError::Tool("empty embedding returned".into()));
    }

    Ok(embedding)
}

impl Default for KnowledgeBaseTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for KnowledgeBaseTool {
    fn name(&self) -> &str {
        "knowledge_base"
    }

    fn description(&self) -> &str {
        "Query and store information in the agent's knowledge base using vector similarity search (RAG). Requires pgvector-enabled PostgreSQL."
    }

    fn primitive(&self) -> ActionPrimitive {
        ActionPrimitive::Read
    }
    fn risk(&self) -> RiskLevel {
        RiskLevel::Low
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "knowledge_base".into(),
            description: self.description().into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["query", "store", "delete"],
                        "description": "Operation to perform"
                    },
                    "query": {
                        "type": "string",
                        "description": "Natural language query (for 'query' operation)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Text content to store (for 'store' operation)"
                    },
                    "entry_id": {
                        "type": "string",
                        "description": "Entry ID to delete (for 'delete' operation)"
                    },
                    "top_k": {
                        "type": "integer",
                        "description": "Number of results to return (default: 5)"
                    },
                    "namespace": {
                        "type": "string",
                        "description": "Knowledge namespace for scoping (default: 'default')"
                    },
                    "metadata": {
                        "type": "object",
                        "description": "Metadata to attach to stored entry"
                    }
                },
                "required": ["operation"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult, ClawzError> {
        let operation = args["operation"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("operation required".into()))?;

        let namespace = args["namespace"].as_str();

        let output = match operation {
            "query" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| ClawzError::Validation("query required".into()))?;

                let top_k = args["top_k"].as_u64().unwrap_or(5) as usize;

                let entries = Self::query_rag(query, top_k, &ctx.agent_id, namespace).await?;

                serde_json::json!({
                    "query": query,
                    "results": entries,
                    "count": entries.len()
                })
                .to_string()
            }

            "store" => {
                let content = args["content"]
                    .as_str()
                    .ok_or_else(|| ClawzError::Validation("content required".into()))?;

                let metadata = args["metadata"].clone();

                let id = Self::store_entry(content, metadata, &ctx.agent_id, namespace).await?;

                serde_json::json!({
                    "stored": true,
                    "id": id,
                    "content_length": content.len()
                })
                .to_string()
            }

            "delete" => {
                let entry_id = args["entry_id"]
                    .as_str()
                    .ok_or_else(|| ClawzError::Validation("entry_id required".into()))?;

                let deleted = Self::delete_entry(entry_id, &ctx.agent_id).await?;

                serde_json::json!({
                    "deleted": deleted,
                    "id": entry_id
                })
                .to_string()
            }

            other => {
                return Err(ClawzError::Validation(format!(
                    "unknown operation: {}",
                    other
                )));
            }
        };

        Ok(ToolResult {
            tool_call_id: String::new(),
            output,
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_knowledge_base_name() {
        assert_eq!(KnowledgeBaseTool::new().name(), "knowledge_base");
    }

    #[test]
    fn test_knowledge_base_schema() {
        let schema = KnowledgeBaseTool::new().schema();
        assert_eq!(schema.name, "knowledge_base");
        let ops = &schema.parameters["properties"]["operation"]["enum"];
        assert!(
            ops.as_array()
                .unwrap()
                .contains(&serde_json::json!("query"))
        );
        assert!(
            ops.as_array()
                .unwrap()
                .contains(&serde_json::json!("store"))
        );
        assert!(
            ops.as_array()
                .unwrap()
                .contains(&serde_json::json!("delete"))
        );
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let tool = KnowledgeBaseTool::new();
        let ctx = ToolContext {
            agent_id: "a".into(),
            conversation_id: "c".into(),
            user_id: None,
            config: crate::tools::ToolConfig::default(),
        };
        let result = tool.execute(&ctx, serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_query_without_db() {
        // When DB is not configured, should return Config error
        let tool = KnowledgeBaseTool::new();
        let ctx = ToolContext {
            agent_id: "a".into(),
            conversation_id: "c".into(),
            user_id: None,
            config: crate::tools::ToolConfig::default(),
        };
        if std::env::var("CLAWZ_DB_URL").is_err() && std::env::var("DATABASE_URL").is_err() {
            let result = tool
                .execute(
                    &ctx,
                    serde_json::json!({"operation": "query", "query": "test"}),
                )
                .await;
            assert!(result.is_err());
        }
    }
}
