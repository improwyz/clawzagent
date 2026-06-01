// swarm_acceptance.rs — PRISM-G Swarm dimension acceptance tests

use clawz_core::types::orchestration::ToolType;
use uuid::Uuid;

use clawz_worker::orchestration::tool_orchestrator::InMemoryToolOrchestrator;
use clawz_worker::runtime::swarm::{
    ConflictResolver, DependencyGraph, ResolutionStrategy, SwarmPattern,
};

fn make_spawn_config(owner_agent_id: &str) -> clawz_core::types::orchestration::SpawnConfig {
    use clawz_core::types::orchestration::SpawnConfig;
    SpawnConfig {
        memory_mb: 256,
        cpu_millicores: 500,
        image: "".to_string(),
        env: Vec::new(),
        labels: vec![("owner-agent-id".to_string(), owner_agent_id.to_string())],
        network: "bridge".to_string(),
    }
}

// ---------------------------------------------------------------------------
// InMemoryToolOrchestrator tests
// ---------------------------------------------------------------------------

#[test]
fn in_memory_tool_orchestrator_spawn_reap() {
    let orch = InMemoryToolOrchestrator::new();
    let config = make_spawn_config("t1");

    let handle = orch.spawn_tool_sync(ToolType::Browser, config).unwrap();

    assert_ne!(handle.id, Uuid::nil());
    assert_eq!(handle.tool_type, ToolType::Browser);
    assert_eq!(handle.owner_agent_id, "t1");

    let listed = orch.list_tools_sync().unwrap();
    assert_eq!(listed.len(), 1);

    orch.reap_tool_sync(&handle).unwrap();
    assert_eq!(orch.list_tools_sync().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// DependencyGraph tests
// ---------------------------------------------------------------------------

#[test]
fn dependency_graph_critical_path() {
    let mut graph = DependencyGraph::new();
    graph.add_task("A".into(), vec![], 2);
    graph.add_task("B".into(), vec!["A".into()], 3);
    graph.add_task("C".into(), vec!["A".into()], 5);
    graph.add_task("D".into(), vec!["B".into(), "C".into()], 2);
    // A→C→D: 2+5+2=9 (longest)
    // A→B→D: 2+3+2=7 (shorter)
    // Critical path is A, C, D (NOT B)

    let path = graph.critical_path();
    assert!(
        path.contains(&"A".to_string()),
        "A must be on critical path"
    );
    assert!(
        path.contains(&"C".to_string()),
        "C must be on critical path"
    );
    assert!(
        path.contains(&"D".to_string()),
        "D must be on critical path"
    );
    assert!(
        !path.contains(&"B".to_string()),
        "B must NOT be on critical path"
    );
}

// ---------------------------------------------------------------------------
// ConflictResolver tests
// ---------------------------------------------------------------------------

#[test]
fn conflict_resolver_voting() {
    let resolver = ConflictResolver::new(ResolutionStrategy::Voting);
    let options = vec![
        "option_a".to_string(),
        "option_b".to_string(),
        "option_a".to_string(),
    ];
    assert_eq!(resolver.resolve(&options), "option_a");
}

#[test]
fn conflict_resolver_escalation() {
    let resolver = ConflictResolver::new(ResolutionStrategy::Escalation);
    let options = vec!["opt1".to_string(), "opt2".to_string()];
    assert_eq!(resolver.resolve(&options), "ESCALATE");
}

// ---------------------------------------------------------------------------
// SwarmPattern serde test
// ---------------------------------------------------------------------------

#[test]
fn swarm_pattern_serde() {
    let p = SwarmPattern::PeerToPeer;
    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(json, "\"peer_to_peer\"");
    let back: SwarmPattern = serde_json::from_str(&json).unwrap();
    assert_eq!(back, p);
}
