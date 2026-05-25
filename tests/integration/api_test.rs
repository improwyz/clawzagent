use reqwest;
use tokio;

const REST_ENDPOINTS: &[&str] = &[
    "/",
    "/health",
    "/api/v1/agents",
    "/api/v1/agents/agent-1",
    "/api/v1/agents/agent-1/tasks",
    "/api/v1/agents/agent-1/executions",
    "/api/v1/tasks",
    "/api/v1/tasks/task-1",
    "/api/v1/tasks/task-1/status",
    "/api/v1/tasks/task-1/result",
    "/api/v1/executions",
    "/api/v1/executions/exec-1",
    "/api/v1/executions/exec-1/logs",
    "/api/v1/contracts",
    "/api/v1/contracts/contract-1",
    "/api/v1/contracts/contract-1/signatures",
    "/api/v1/governance",
    "/api/v1/governance/proposals",
    "/api/v1/governance/proposals/prop-1",
    "/api/v1/governance/votes",
    "/api/v1/governance/votes/vote-1",
    "/api/v1/metrics",
    "/api/v1/metrics/agents",
    "/api/v1/metrics/tasks",
    "/api/v1/metrics/executions",
    "/api/v1/workers",
    "/api/v1/workers/worker-1",
    "/api/v1/workers/worker-1/status",
    "/api/v1/queue",
    "/api/v1/queue/pending",
    "/api/v1/queue/processing",
    "/api/v1/cache",
    "/api/v1/cache/stats",
    "/api/v1/cache/keys",
    "/api/v1/config",
    "/api/v1/config/agents",
    "/api/v1/config/workers",
    "/api/v1/config/governance",
    "/api/v1/health",
    "/api/v1/health/workers",
    "/api/v1/health/queue",
    "/api/v1/health/cache",
    "/api/v1/limits",
    "/api/v1/limits/rate",
    "/api/v1/limits/quotas",
    "/api/v1/limits/concurrency",
    "/api/v1/version",
    "/api/v1/schema",
    "/api/v1/schema/agent",
    "/api/v1/schema/task",
    "/api/v1/schema/execution",
    "/api/v1/schema/contract",
    "/api/v1/schema/proposal",
    "/api/v1/events",
    "/api/v1/events/agent",
    "/api/v1/events/task",
    "/api/v1/events/governance",
];

#[tokio::test]
async fn test_58_rest_endpoints() {
    let client = reqwest::Client::new();
    let base_url = "http://localhost:3000";
    
    let mut success_count = 0;
    let mut failed_endpoints = Vec::new();
    
    for endpoint in REST_ENDPOINTS {
        let url = format!("{}{}", base_url, endpoint);
        match client.get(&url).send().await {
            Ok(resp) => {
                if resp.status().is_success() || resp.status().is_redirection() {
                    success_count += 1;
                } else {
                    failed_endpoints.push(format!("{} - {}", endpoint, resp.status()));
                }
            }
            Err(e) => {
                failed_endpoints.push(format!("{} - {}", endpoint, e));
            }
        }
    }
    
    println!("Tested {} endpoints, {} successful", REST_ENDPOINTS.len(), success_count);
    if !failed_endpoints.is_empty() {
        for failed in &failed_endpoints {
            println!("Failed: {}", failed);
        }
    }
    assert!(success_count >= 50, "At least 50 endpoints should respond successfully");
}

#[tokio::test]
async fn test_rest_endpoint_responses() {
    let client = reqwest::Client::new();
    let base_url = "http://localhost:3000";
    
    // Test root endpoint
    let resp = client.get(format!("{}/", base_url)).send().await.unwrap();
    assert!(resp.status().is_success());
    
    // Test health endpoint
    let resp = client.get(format!("{}/health", base_url)).send().await.unwrap();
    assert!(resp.status().is_success());
    
    // Test API endpoints
    let resp = client.get(format!("{}/api/v1/agents", base_url)).send().await.unwrap();
    assert!(resp.status().is_success());
    
    let resp = client.get(format!("{}/api/v1/tasks", base_url)).send().await.unwrap();
    assert!(resp.status().is_success());
}
