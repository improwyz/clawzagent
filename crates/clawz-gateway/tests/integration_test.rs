use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use clawz_gateway::{AppState, server::GatewayServer};
use tower::ServiceExt;

fn make_server() -> GatewayServer {
    let state = AppState::new("test-secret-key");
    GatewayServer::new(state)
}

#[tokio::test]
async fn test_health() {
    let server = make_server();
    let response = server
        .app
        .oneshot(
            Request::builder()
                .uri("/api/v1/system/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_agents_list() {
    let server = make_server();
    let response = server
        .app
        .oneshot(
            Request::builder()
                .uri("/api/v1/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_conversations_list() {
    let server = make_server();
    let response = server
        .app
        .oneshot(
            Request::builder()
                .uri("/api/v1/conversations")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_channels_list() {
    let server = make_server();
    let response = server
        .app
        .oneshot(
            Request::builder()
                .uri("/api/v1/channels")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_providers_list() {
    let server = make_server();
    let response = server
        .app
        .oneshot(
            Request::builder()
                .uri("/api/v1/providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_tools_list() {
    let server = make_server();
    let response = server
        .app
        .oneshot(
            Request::builder()
                .uri("/api/v1/tools")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_governance_policies() {
    let server = make_server();
    let response = server
        .app
        .oneshot(
            Request::builder()
                .uri("/api/v1/governance/policies")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_fleet_list() {
    let server = make_server();
    let response = server
        .app
        .oneshot(
            Request::builder()
                .uri("/api/v1/fleet")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_system_metrics() {
    let server = make_server();
    let response = server
        .app
        .oneshot(
            Request::builder()
                .uri("/api/v1/system/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_agent_not_found() {
    let server = make_server();
    let response = server
        .app
        .oneshot(
            Request::builder()
                .uri("/api/v1/agents/nonexistent-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_create_agent() {
    let server = make_server();
    let body = serde_json::json!({
        "name": "test-agent",
        "model": "gpt-4",
        "system_prompt": "You are a helpful assistant."
    });
    let response = server
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agents")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_websocket_endpoints_reject_http() {
    let server = make_server();
    let ws_routes = vec![
        "/ws/agent/test-123/stream",
        "/ws/events",
        "/ws/metrics",
        "/ws/approvals",
        "/ws/logs",
        "/ws/voice",
    ];
    for route in ws_routes {
        let response = server
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(route)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            response.status() == StatusCode::BAD_REQUEST
                || response.status() == StatusCode::UPGRADE_REQUIRED
                || response.status() == StatusCode::METHOD_NOT_ALLOWED,
            "WS route {} should reject plain HTTP, got {}",
            route,
            response.status()
        );
    }
}

/// Verifies that `POST /api/v1/agents/{id}/autonomous` starts a multi-turn
/// session and returns a session_id.
///
/// The endpoint accepts `maxTurns`, `costBudgetUsd`, and optional
/// `systemPrompt` overrides. It creates an `AutonomousSessionRecord` in
/// `AppState.autonomous_sessions` and returns the freshly generated session ID
/// so callers can subscribe to the WebSocket activity stream.
#[tokio::test]
async fn autonomous_endpoint_starts_multi_turn_session() {
    let server = make_server();
    // Create an agent first so the autonomous endpoint has a valid target.
    let create_body = serde_json::json!({
        "name": "auto-agent",
        "model": "gpt-4",
        "system_prompt": "you are a helpful assistant"
    });
    let created = server
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agents")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(created.into_body(), 64 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let agent_id = v["id"].as_str().unwrap().to_string();

    // Now invoke the autonomous endpoint.
    let response = server
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/agents/{}/autonomous", agent_id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{ "maxTurns": 10, "costBudgetUsd": 0.50 }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v["session_id"].is_string(), "expected session_id, got {v:?}");
    assert_eq!(v["status"], "running");
}

/// Smoke test: the autonomous-activity WebSocket route is registered.
///
/// We do not perform a full WebSocket handshake here (Axum requires the upgrade
/// headers and tokio-tungstenite for that). Instead we send a plain HTTP GET
/// and assert the response is one of the documented "missing upgrade" codes,
/// proving the router has the route bound rather than 404-ing.
#[tokio::test]
async fn ws_autonomous_stream_endpoint_exists() {
    let server = make_server();
    let response = server
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/ws/agents/some-agent-id/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // The route must exist: anything other than NOT_FOUND proves the WS route
    // was matched. Axum returns BAD_REQUEST when WebSocket upgrade headers are
    // missing, or UPGRADE_REQUIRED on some configurations.
    assert_ne!(
        response.status(),
        StatusCode::NOT_FOUND,
        "/ws/agents/{{id}}/stream must be registered as a route"
    );
    assert!(
        response.status() == StatusCode::BAD_REQUEST
            || response.status() == StatusCode::UPGRADE_REQUIRED
            || response.status() == StatusCode::METHOD_NOT_ALLOWED,
        "/ws/agents/{{id}}/stream should reject plain HTTP, got {}",
        response.status()
    );
}
