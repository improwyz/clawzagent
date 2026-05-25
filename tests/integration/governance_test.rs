use reqwest;
use tokio;

#[tokio::test]
async fn test_governance_endpoints() {
    let client = reqwest::Client::new();
    let base_url = "http://localhost:3000";
    
    // Test governance main endpoint
    let resp = client.get(format!("{}/api/v1/governance", base_url)).send().await.unwrap();
    assert!(resp.status().is_success());
    
    // Test proposals endpoint
    let resp = client.get(format!("{}/api/v1/governance/proposals", base_url)).send().await.unwrap();
    assert!(resp.status().is_success());
    
    // Test votes endpoint
    let resp = client.get(format!("{}/api/v1/governance/votes", base_url)).send().await.unwrap();
    assert!(resp.status().is_success());
    
    // Test contracts endpoint
    let resp = client.get(format!("{}/api/v1/contracts", base_url)).send().await.unwrap();
    assert!(resp.status().is_success());
}

#[tokio::test]
async fn test_governance_websocket() {
    let client = reqwest::Client::new();
    let base_url = "http://localhost:3000";
    
    // Test governance WS channel
    let resp = client.get(format!("{}/ws/governance", base_url)).send().await.unwrap();
    // Should either succeed or return 400 (not a real WS upgrade in test)
    assert!(resp.status().is_success() || resp.status().is_bad_request());
}
