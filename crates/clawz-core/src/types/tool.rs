//! Tool types — schema, call, result, and registration.
//!
//! Tools are executable capabilities that agents can invoke. Each tool
//! exposes a JSON Schema describing its arguments so the LLM can decide
//! when and how to call it.
//!
//! // Dependency: used by worker::tool implementations, gateway::tool_registry

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// ── ToolCall / ToolResult ─────────────────────────────────────────────────────

/// A request from the LLM to invoke a tool.
/// // Dependency: embedded in MessageContent::ToolCalls, processed by worker::tool_orchestrator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique call id assigned by the model (used to correlate results).
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }
}

/// The outcome of a tool execution, returned to the LLM.
/// // Dependency: embedded in MessageContent::ToolResult.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    /// Serialised output (string for text, JSON string for structured data).
    pub output: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(tool_call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            output: output.into(),
            is_error: false,
        }
    }

    pub fn err(tool_call_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            output: error.into(),
            is_error: true,
        }
    }
}

// ── ToolSchema ────────────────────────────────────────────────────────────────

/// JSON-Schema-compatible description of a tool exposed to LLMs.
/// // Dependency: returned by traits::Tool::schema, sent in ChatRequest.tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    /// JSON Schema object for the tool's `arguments` parameter.
    pub parameters: Value,
}

impl ToolSchema {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// Build a simple no-argument schema.
    pub fn no_args(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(
            name,
            description,
            serde_json::json!({ "type": "object", "properties": {}, "required": [] }),
        )
    }
}

// ── ToolRegistration ──────────────────────────────────────────────────────────

/// Registered tool entry in the platform registry.
/// // Dependency: stored in worker::tool_registry, exposed by gateway::tool_handlers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRegistration {
    pub id: Uuid,
    pub schema: ToolSchema,
    pub enabled: bool,
    /// Logical grouping (e.g. "web", "code", "system").
    pub category: String,
    pub version: String,
}

impl ToolRegistration {
    pub fn new(schema: ToolSchema, category: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            schema,
            enabled: true,
            category: category.into(),
            version: "1.0.0".into(),
        }
    }

    pub fn disable(mut self) -> Self {
        self.enabled = false;
        self
    }
}
