//! Runtime capability registry for semantic tool discovery.
//!
//! Agents can query this registry at runtime to discover tools by
//! keyword overlap against tool descriptions, enabling dynamic tool
//! composition without hard-coded tool lists.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use serde_json::Value;
use clawz_core::error::ClawzError;
use crate::tools::tool_trait::Tool;
use crate::tools::builtin;

/// A tool's capability descriptor for semantic matching.
#[derive(Debug, Clone)]
pub struct ToolCapability {
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub input_schema: Value,
    pub risk_level: clawz_core::types::tool_risk::RiskLevel,
}

/// Registry enabling agents to discover and compose tools by semantic query at runtime.
pub struct ToolCapabilityRegistry {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    keyword_index: RwLock<HashMap<String, Vec<String>>>, // keyword -> tool names
}

impl Default for ToolCapabilityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolCapabilityRegistry {
    pub fn new() -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            keyword_index: RwLock::new(HashMap::new()),
        }
    }

    /// Register a tool with the registry.
    pub fn register(&self, name: &str, tool: Arc<dyn Tool>) -> Result<(), ClawzError> {
        let name = name.to_string();
        let description = tool.description().to_string();
        let keywords = Self::extract_keywords(&description);
        let _input_schema = tool.schema().parameters;
        let _risk_level = tool.risk();

        {
            let mut tools = self.tools.write().unwrap();
            tools.insert(name.clone(), tool);
        }

        {
            let mut index = self.keyword_index.write().unwrap();
            for kw in &keywords {
                index.entry(kw.clone()).or_default().push(name.clone());
            }
        }

        Ok(())
    }

    /// Register all built-in tools (calculator, browser, file_ops, git, shell,
    /// web_search, web_fetch, pdf_read, image_gen, knowledge_base, escalate).
    pub fn register_builtins(&self) -> Result<(), ClawzError> {
        let builtins: Vec<(&str, Arc<dyn Tool>)> = vec![
            ("calculator", Arc::new(builtin::calculator::CalculatorTool::new()) as Arc<dyn Tool>),
            ("browser",    Arc::new(builtin::browser::BrowserTool::new())),
            ("file_ops",   Arc::new(builtin::file_ops::FileOpsTool::new())),
            ("git",        Arc::new(builtin::git::GitTool::new())),
            ("shell",      Arc::new(builtin::shell::ShellTool::new())),
            ("web_search", Arc::new(builtin::web_search::WebSearchTool::new())),
            ("web_fetch",  Arc::new(builtin::web_fetch::WebFetchTool::new())),
            ("pdf_read",   Arc::new(builtin::pdf_read::PdfReadTool::new())),
            ("image_gen",  Arc::new(builtin::image_gen::ImageGenTool::new())),
            ("knowledge_base", Arc::new(builtin::knowledge_base::KnowledgeBaseTool::new())),
            ("escalate",   Arc::new(builtin::escalate::EscalateTool::new())),
        ];

        for (name, tool) in builtins {
            self.register(name, tool)?;
        }
        Ok(())
    }

    /// Find tool names whose descriptions match the query string via keyword overlap.
    ///
    /// Tokenizes the query into lowercase words, filters stopwords, then scores
    /// each registered tool by the number of query keywords present in its keyword set.
    /// Returns all matching tools sorted by descending score.
    pub fn find_matching(&self, query: &str) -> Vec<ToolCapability> {
        let query_keywords: Vec<String> = Self::extract_keywords(query);
        if query_keywords.is_empty() {
            return vec![];
        }

        let tools = self.tools.read().unwrap();
        let mut scored: Vec<(i64, ToolCapability)> = Vec::new();

        for (name, tool) in tools.iter() {
            let cap = self.build_capability(name, tool);
            let score = cap.keywords.iter()
                .filter(|kw| query_keywords.contains(kw))
                .count() as i64;
            if score > 0 {
                scored.push((score, cap));
            }
        }

        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, cap)| cap).collect()
    }

    /// List all registered capability names.
    pub fn list_all(&self) -> Vec<String> {
        let tools = self.tools.read().unwrap();
        tools.keys().cloned().collect()
    }

    /// Get the full Tool handle for a named capability.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        let tools = self.tools.read().unwrap();
        tools.get(name).cloned()
    }

    /// Build a ToolCapability for a registered tool.
    fn build_capability(&self, name: &str, tool: &Arc<dyn Tool>) -> ToolCapability {
        let description = tool.description().to_string();
        ToolCapability {
            name: name.to_string(),
            description: description.clone(),
            keywords: Self::extract_keywords(&description),
            input_schema: tool.schema().parameters.clone(),
            risk_level: tool.risk(),
        }
    }

    /// Tokenize description into lowercase keywords, filtering stopwords.
    fn extract_keywords(description: &str) -> Vec<String> {
        const STOPWORDS: &[&str] = &[
            "the", "a", "an", "is", "are", "to", "for", "of", "and",
            "or", "in", "on", "with", "as", "by", "from", "that",
            "this", "it", "be", "have", "has", "was", "were", "will",
        ];

        description
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|word| {
                let word = word.trim();
                !word.is_empty() && word.len() > 1 && !STOPWORDS.contains(&word)
            })
            .map(|word| word.to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_registry() -> ToolCapabilityRegistry {
        let reg = ToolCapabilityRegistry::new();
        reg.register_builtins().unwrap();
        reg
    }

    #[test]
    fn registry_lists_all_builtins() {
        let reg = make_registry();
        let names = reg.list_all();
        assert!(!names.is_empty(), "builtins should be registered");
        for expected in ["calculator", "browser", "file_ops", "git", "shell",
                         "web_search", "web_fetch", "pdf_read", "image_gen",
                         "knowledge_base", "escalate"] {
            assert!(names.iter().any(|n| n.as_str() == expected),
                    "expected '{expected}' in builtin list");
        }
    }

    #[test]
    fn find_matching_finds_calculator_for_math_query() {
        let reg = make_registry();
        let results = reg.find_matching("math arithmetic calculate");
        assert!(!results.is_empty(), "should find calculator for math query");
        assert!(results.iter().any(|c| c.name == "calculator"),
                "calculator should be top match for math query");
    }

    #[test]
    fn find_matching_finds_browser_for_web_query() {
        let reg = make_registry();
        let results = reg.find_matching("web browse internet");
        assert!(!results.is_empty(), "should find browser for web query");
        assert!(results.iter().any(|c| c.name == "browser" || c.name == "web_search" || c.name == "web_fetch"),
                "browser/web tool should match web query");
    }

    #[test]
    fn get_returns_tool_for_known_name() {
        let reg = make_registry();
        let tool = reg.get("calculator");
        assert!(tool.is_some(), "calculator should be retrievable");
        assert_eq!(tool.unwrap().name(), "calculator");
    }

    #[test]
    fn get_returns_none_for_unknown_name() {
        let reg = make_registry();
        assert!(reg.get("nonexistent_tool").is_none());
    }

    #[test]
    fn find_matching_returns_empty_for_stopword_only_query() {
        let reg = make_registry();
        let results = reg.find_matching("the a an");
        assert!(results.is_empty(), "stopword-only query should return empty");
    }
}