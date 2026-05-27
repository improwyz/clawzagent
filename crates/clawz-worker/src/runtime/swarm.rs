// swarm.rs — PRISM-G Swarm dimension: agent roles, collaboration patterns, dependency graphs

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use clawz_core::types::tool_risk::RiskLevel;

// ---------------------------------------------------------------------------
// Swarm patterns
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwarmPattern {
    Hierarchical,
    Pipeline,
    Competitive,
    PeerToPeer,
    Adaptive,
}

// ---------------------------------------------------------------------------
// AgentRole
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRole {
    pub role_id: String,
    pub name: String,
    pub capabilities: Vec<String>,
    pub knowledge_domains: Vec<String>,
    pub tools: Vec<String>,
    pub collaborates_with: Vec<String>,
    pub risk_level: RiskLevel,
}

// ---------------------------------------------------------------------------
// ConflictResolver
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionStrategy {
    Voting,
    Authority,
    Arbitration,
    Escalation,
}

pub struct ConflictResolver {
    strategy: ResolutionStrategy,
}

impl ConflictResolver {
    pub fn new(strategy: ResolutionStrategy) -> Self {
        Self { strategy }
    }

    pub fn resolve(&self, options: &[String]) -> String {
        match self.strategy {
            ResolutionStrategy::Voting => {
                let mut counts: HashMap<&str, usize> = HashMap::new();
                for opt in options {
                    *counts.entry(opt.as_str()).or_insert(0) += 1;
                }
                counts
                    .into_iter()
                    .max_by_key(|(_, c)| *c)
                    .map(|(k, _)| k.to_string())
                    .unwrap_or_default()
            }
            ResolutionStrategy::Authority => {
                options.first().cloned().unwrap_or_default()
            }
            ResolutionStrategy::Arbitration => {
                options.get(options.len() / 2).cloned().unwrap_or_default()
            }
            ResolutionStrategy::Escalation => "ESCALATE".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// DependencyGraph
// ---------------------------------------------------------------------------

pub struct DependencyGraph {
    edges: HashMap<String, Vec<String>>,
    durations: HashMap<String, u64>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self {
            edges: HashMap::new(),
            durations: HashMap::new(),
        }
    }

    pub fn add_task(&mut self, id: String, depends_on: Vec<String>, duration: u64) {
        self.edges.insert(id.clone(), depends_on);
        self.durations.insert(id, duration);
    }

    /// Topological sort via Kahn's algorithm.
    pub fn execution_order(&self) -> Result<Vec<String>, String> {
        let mut dep_count: HashMap<String, usize> = HashMap::new();
        for (node, deps) in &self.edges {
            dep_count.insert(node.clone(), deps.len());
        }

        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        for (node, deps) in &self.edges {
            for dep in deps {
                dependents.entry(dep.clone()).or_default().push(node.clone());
            }
        }

        let mut zero_deps: Vec<String> = Vec::new();
        for (k, &c) in &dep_count {
            if c == 0 {
                zero_deps.push(k.clone());
            }
        }
        let mut queue: VecDeque<String> = VecDeque::from(zero_deps);
        let mut order = Vec::new();

        while let Some(n) = queue.pop_front() {
            order.push(n.clone());
            if let Some(deps) = dependents.get(&n) {
                for dependent in deps {
                    let cnt = dep_count.get_mut(dependent).unwrap();
                    *cnt = cnt.saturating_sub(1);
                    if *cnt == 0 {
                        queue.push_back(dependent.clone());
                    }
                }
            }
        }

        if order.len() != self.edges.len() {
            Err("cycle detected".into())
        } else {
            Ok(order)
        }
    }

    /// Critical path — nodes that lie on at least one longest-duration root-to-leaf path.
    ///
    /// Let:
    /// - dist_to_leaf[n] = longest duration from n to any leaf node (n's own duration included).
    ///   Computed by processing nodes in reverse topological order.
    /// - dist_from_root[n] = longest duration from any root to n (n's own duration included).
    ///   Computed by processing nodes in forward topological order.
    /// - L = max(dist_to_leaf.values()) = longest root-to-leaf path length.
    ///
    /// A node n is on a critical path iff:
    ///   dist_from_root[n] + dist_to_leaf[n] - duration[n] == L
    /// (Equivalently: there exists a root-to-leaf path of length L that passes through n.)
    pub fn critical_path(&self) -> Vec<String> {
        let dependents: HashMap<String, Vec<String>> =
            self.edges
                .iter()
                .fold(HashMap::new(), |mut acc, (node, deps)| {
                    for dep in deps {
                        acc.entry(dep.clone())
                            .or_insert_with(Vec::new)
                            .push(node.clone());
                    }
                    acc
                });

        let topo = match self.execution_order() {
            Ok(t) => t,
            Err(_) => return Vec::new(),
        };

        // dist_to_leaf[n] (reverse topo pass)
        let mut dist_to_leaf: HashMap<String, i64> = HashMap::new();
        for node in topo.iter().rev() {
            let dur = *self.durations.get(node).unwrap_or(&1) as i64;
            if let Some(node_deps) = dependents.get(node) {
                let max_child = node_deps
                    .iter()
                    .map(|d| *dist_to_leaf.get(d).unwrap_or(&0))
                    .max()
                    .unwrap_or(0);
                dist_to_leaf.insert(node.clone(), dur + max_child);
            } else {
                dist_to_leaf.insert(node.clone(), dur);
            }
        }

        // dist_from_root[n] (forward topo pass)
        let mut dist_from_root: HashMap<String, i64> = HashMap::new();
        for node in &topo {
            let dur = *self.durations.get(node).unwrap_or(&1) as i64;
            let node_deps = self.edges.get(node);
            match node_deps {
                None => {
                    dist_from_root.insert(node.clone(), dur);
                }
                Some(deps) if deps.is_empty() => {
                    dist_from_root.insert(node.clone(), dur);
                }
                Some(deps) => {
                    let max_from_pred = deps
                        .iter()
                        .map(|p| dist_from_root.get(p).copied().unwrap_or(0))
                        .max()
                        .unwrap_or(0);
                    dist_from_root.insert(node.clone(), max_from_pred + dur);
                }
            }
        }

        let max_path_len = dist_to_leaf.values().copied().max().unwrap_or(0);

        topo.into_iter()
            .filter(|node| {
                let from_root = dist_from_root.get(node).copied().unwrap_or(0);
                let to_leaf = dist_to_leaf.get(node).copied().unwrap_or(0);
                let node_dur = *self.durations.get(node).unwrap_or(&1) as i64;
                from_root + to_leaf - node_dur == max_path_len
            })
            .collect()
    }
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Inline tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swarm_pattern_serde_hierarchical() {
        let p = SwarmPattern::Hierarchical;
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"hierarchical\"");
        let back: SwarmPattern = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn swarm_pattern_serde_pipeline() {
        let p = SwarmPattern::Pipeline;
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"pipeline\"");
        let back: SwarmPattern = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn swarm_pattern_serde_competitive() {
        let p = SwarmPattern::Competitive;
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"competitive\"");
        let back: SwarmPattern = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn swarm_pattern_serde_peer_to_peer() {
        let p = SwarmPattern::PeerToPeer;
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"peer_to_peer\"");
        let back: SwarmPattern = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn swarm_pattern_serde_adaptive() {
        let p = SwarmPattern::Adaptive;
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"adaptive\"");
        let back: SwarmPattern = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn conflict_resolver_voting() {
        let resolver = ConflictResolver::new(ResolutionStrategy::Voting);
        let options = vec!["opt_a".to_string(), "opt_b".to_string(), "opt_a".to_string()];
        assert_eq!(resolver.resolve(&options), "opt_a");
    }

    #[test]
    fn conflict_resolver_authority() {
        let resolver = ConflictResolver::new(ResolutionStrategy::Authority);
        let options = vec!["opt_first".to_string(), "opt_second".to_string()];
        assert_eq!(resolver.resolve(&options), "opt_first");
    }

    #[test]
    fn conflict_resolver_authority_empty() {
        let resolver = ConflictResolver::new(ResolutionStrategy::Authority);
        let options: Vec<String> = vec![];
        assert_eq!(resolver.resolve(&options), "");
    }

    #[test]
    fn conflict_resolver_arbitration() {
        let resolver = ConflictResolver::new(ResolutionStrategy::Arbitration);
        let options = vec!["lo".to_string(), "mid".to_string(), "hi".to_string()];
        assert_eq!(resolver.resolve(&options), "mid");
    }

    #[test]
    fn conflict_resolver_escalation() {
        let resolver = ConflictResolver::new(ResolutionStrategy::Escalation);
        let options = vec!["opt1".to_string(), "opt2".to_string()];
        assert_eq!(resolver.resolve(&options), "ESCALATE");
    }

    #[test]
    fn dependency_graph_linear_chain() {
        let mut graph = DependencyGraph::new();
        graph.add_task("A".into(), vec![], 2);
        graph.add_task("B".into(), vec!["A".into()], 3);
        graph.add_task("C".into(), vec!["B".into()], 4);

        let order = graph.execution_order().unwrap();
        assert_eq!(order, vec!["A", "B", "C"]);
    }

    #[test]
    fn dependency_graph_fan_out() {
        let mut graph = DependencyGraph::new();
        graph.add_task("A".into(), vec![], 2);
        graph.add_task("B".into(), vec!["A".into()], 3);
        graph.add_task("C".into(), vec!["A".into()], 5);
        graph.add_task("D".into(), vec!["B".into(), "C".into()], 2);

        let order = graph.execution_order().unwrap();
        assert_eq!(order.first(), Some(&"A".to_string()));
        assert!(order.contains(&"D".to_string()));
    }

    #[test]
    fn dependency_graph_critical_path_ab() {
        let mut graph = DependencyGraph::new();
        graph.add_task("A".into(), vec![], 2);
        graph.add_task("B".into(), vec!["A".into()], 3);
        // A→B: path length = 5 (critical path is the whole chain)
        let path = graph.critical_path();
        assert_eq!(path[0], "A");
        assert!(path.contains(&"B".to_string()));
    }

    #[test]
    fn dependency_graph_critical_path_acd() {
        let mut graph = DependencyGraph::new();
        graph.add_task("A".into(), vec![], 2);
        graph.add_task("B".into(), vec!["A".into()], 3);
        graph.add_task("C".into(), vec!["A".into()], 5);
        graph.add_task("D".into(), vec!["B".into(), "C".into()], 2);
        // A→C→D: 2+5+2=9  |  A→B→D: 2+3+2=7  → critical path A,C,D
        let path = graph.critical_path();
        assert_eq!(path[0], "A");
        assert!(path.contains(&"C".to_string()));
        assert!(path.contains(&"D".to_string()));
    }

    #[test]
    fn dependency_graph_cycle_detection() {
        let mut graph = DependencyGraph::new();
        graph.add_task("A".into(), vec!["B".into()], 1);
        graph.add_task("B".into(), vec!["A".into()], 1);

        let result = graph.execution_order();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "cycle detected");
    }

    #[test]
    fn agent_role_serde() {
        let role = AgentRole {
            role_id: "coordinator".to_string(),
            name: "Coordinator".to_string(),
            capabilities: vec!["plan".to_string(), "delegate".to_string()],
            knowledge_domains: vec!["orchestration".to_string()],
            tools: vec!["task_spawn".to_string()],
            collaborates_with: vec!["executor".to_string()],
            risk_level: RiskLevel::Medium,
        };
        let json = serde_json::to_string(&role).unwrap();
        let back: AgentRole = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role_id, "coordinator");
        assert_eq!(back.risk_level, RiskLevel::Medium);
    }

    #[test]
    fn resolution_strategy_serde() {
        let s = ResolutionStrategy::Arbitration;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"arbitration\"");
        let back: ResolutionStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
