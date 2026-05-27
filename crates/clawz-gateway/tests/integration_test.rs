use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use clawz_gateway::{AppState, auth::api_key::ApiKeyValidator, bootstrap, server::GatewayServer};
use clawz_worker::client::InProcessExecutionClient;
use clawz_worker::service::WorkerService;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

static TEST_ENV: Mutex<()> = Mutex::new(());

fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_ENV
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Holds the process env lock for the duration of an integration test.
struct TestServer {
    _env_guard: std::sync::MutexGuard<'static, ()>,
    inner: GatewayServer,
}

impl TestServer {
    fn app(&self) -> axum::Router {
        self.inner.app.clone()
    }
}

fn configure_dev_auth() {
    unsafe {
        std::env::set_var("CLAWZ_DISABLE_AUTH", "1");
    }
}

fn configure_api_key_auth(api_keys: &str) {
    unsafe {
        std::env::set_var("CLAWZ_DISABLE_AUTH", "0");
        std::env::set_var("VALID_API_KEYS", api_keys);
        std::env::set_var("CLAWZ_JWT_SECRET", "test-secret-key");
    }
}

async fn make_server() -> TestServer {
    let guard = test_env_lock();
    configure_dev_auth();
    TestServer {
        _env_guard: guard,
        inner: build_gateway_server().await,
    }
}

async fn make_server_with_api_key_auth(api_keys: &str) -> TestServer {
    let guard = test_env_lock();
    configure_api_key_auth(api_keys);
    TestServer {
        _env_guard: guard,
        inner: build_gateway_server().await,
    }
}

async fn build_gateway_server() -> GatewayServer {
    let (_platform, approval) = bootstrap::build_platform_with_approval()
        .await
        .expect("platform bootstrap");
    let service = Arc::new(
        WorkerService::new_with_approval(approval.clone())
            .await
            .expect("worker service"),
    );
    let platform = clawz_services::Platform::new(Arc::new(InProcessExecutionClient::new(service)));
    let scheduler = bootstrap::build_agent_scheduler().ok();
    let state = AppState::full(
        "test-secret-key",
        None,
        Some(Arc::new(platform)),
        None,
        approval,
        None,
        scheduler,
    );
    GatewayServer::new(state)
}

#[tokio::test]
async fn test_health() {
    let server = make_server().await;
    let response = server
        .app()
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
    let server = make_server().await;
    let response = server
        .app()
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
    let server = make_server().await;
    let response = server
        .app()
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
    let server = make_server().await;
    let response = server
        .app()
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
    let server = make_server().await;
    let response = server
        .app()
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
    let server = make_server().await;
    let response = server
        .app()
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
    let server = make_server().await;
    let response = server
        .app()
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
    let server = make_server().await;
    let response = server
        .app()
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
    let server = make_server().await;
    let response = server
        .app()
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
    let server = make_server().await;
    let response = server
        .app()
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
    let server = make_server().await;
    let body = serde_json::json!({
        "name": "test-agent",
        "model": "gpt-4",
        "system_prompt": "You are a helpful assistant."
    });
    let response = server
        .app()
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
    let server = make_server().await;
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
            .app()
            .oneshot(Request::builder().uri(route).body(Body::empty()).unwrap())
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
    let server = make_server().await;
    // Create an agent first so the autonomous endpoint has a valid target.
    let create_body = serde_json::json!({
        "name": "auto-agent",
        "model": "gpt-4",
        "system_prompt": "you are a helpful assistant"
    });
    let created = server
        .app()
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
        .app()
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/agents/{}/autonomous", agent_id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{ "maxTurns": 10, "costBudgetUsd": 0.50 }"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        v["session_id"].is_string(),
        "expected session_id, got {v:?}"
    );
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
    let server = make_server().await;
    let response = server
        .app()
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

#[tokio::test]
async fn test_cloud_providers_list() {
    let server = make_server().await;
    let response = server
        .app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/cloud/providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let providers = json["data"].as_array().expect("data array");
    assert!(providers.iter().any(|p| p["id"] == "fly_io"));
    assert!(
        providers.len() >= 15,
        "expected deploy provider registry including opentofu"
    );
}

#[tokio::test]
async fn test_rooms_list_empty() {
    let server = make_server().await;
    let response = server
        .app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/rooms")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["rooms"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_create_room_and_send_message_returns_202() {
    let server = make_server().await;

    let agent_body = serde_json::json!({
        "name": "room-leader",
        "model": "stub",
        "system_prompt": "You are a helpful assistant."
    });
    let agent_resp = server
        .app()
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agents")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&agent_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(agent_resp.status(), StatusCode::CREATED);
    let agent_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(agent_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let agent_id = agent_json["id"].as_str().unwrap().to_string();

    let room_body = serde_json::json!({
        "room_type": "one_many",
        "participants": [{ "agent_id": agent_id, "role": "leader" }]
    });
    let room_resp = server
        .app()
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/rooms")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&room_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(room_resp.status(), StatusCode::CREATED);
    let room_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(room_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let room_id = room_json["id"].as_str().unwrap().to_string();
    assert_eq!(room_json["tenant_id"], "default");

    let msg_body = serde_json::json!({ "content": "Hello team" });
    let msg_resp = server
        .app()
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/rooms/{room_id}/messages"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&msg_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(msg_resp.status(), StatusCode::ACCEPTED);
    let msg_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(msg_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(msg_json["status"], "turn_queued");
    assert!(msg_json["message"]["seq"].as_u64().unwrap_or(0) >= 1);

    let list_resp = server
        .app()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/rooms/{room_id}/messages"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_room_tenant_isolation() {
    let raw_a = "tenant-a-key";
    let raw_b = "tenant-b-key";
    let api_keys = format!(
        "{}:alice:owner:tenant-a,{}:bob:owner:tenant-b",
        ApiKeyValidator::hash_key(raw_a),
        ApiKeyValidator::hash_key(raw_b),
    );
    let server = make_server_with_api_key_auth(&api_keys).await;

    let room_body = serde_json::json!({ "room_type": "many_one" });
    let create = server
        .app()
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/rooms")
                .header("content-type", "application/json")
                .header("x-api-key", raw_a)
                .body(Body::from(serde_json::to_string(&room_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(create.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let room_id = created["id"].as_str().unwrap();
    assert_eq!(created["tenant_id"], "tenant-a");

    let cross_tenant = server
        .app()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/rooms/{room_id}"))
                .header("x-api-key", raw_b)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_tenant.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_channel_tenant_isolation() {
    let raw_a = "tenant-a-key";
    let raw_b = "tenant-b-key";
    let api_keys = format!(
        "{}:alice:owner:tenant-a,{}:bob:owner:tenant-b",
        ApiKeyValidator::hash_key(raw_a),
        ApiKeyValidator::hash_key(raw_b),
    );
    let server = make_server_with_api_key_auth(&api_keys).await;

    let channel_body = serde_json::json!({
        "name": "tenant-a-webhook",
        "channel_type": "webhook",
        "config": { "url": "https://example.com/a" }
    });
    let create = server
        .app()
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/channels")
                .header("content-type", "application/json")
                .header("x-api-key", raw_a)
                .body(Body::from(serde_json::to_string(&channel_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(create.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let channel_id = created["id"].as_str().unwrap();
    assert_eq!(created["tenant_id"], "tenant-a");

    let cross_tenant = server
        .app()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/channels/{channel_id}"))
                .header("x-api-key", raw_b)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_tenant.status(), StatusCode::NOT_FOUND);

    let tenant_b_list = server
        .app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/channels")
                .header("x-api-key", raw_b)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tenant_b_list.status(), StatusCode::OK);
    let list_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(tenant_b_list.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(list_json["total"], 0);
}

#[tokio::test]
async fn test_create_conversation_links_direct_room() {
    let server = make_server().await;

    let agent_body = serde_json::json!({
        "name": "conv-agent",
        "model": "stub",
        "system_prompt": "You are helpful."
    });
    let agent_resp = server
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agents")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&agent_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(agent_resp.status(), StatusCode::CREATED);
    let agent_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(agent_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let agent_id = agent_json["id"].as_str().unwrap();

    let conv_body = serde_json::json!({ "agent_id": agent_id, "title": "Test thread" });
    let conv_resp = server
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/conversations")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&conv_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(conv_resp.status(), StatusCode::CREATED);
    let conv_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(conv_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let conv_id = conv_json["id"].as_str().unwrap();
    assert_eq!(conv_json["room_id"], conv_id);

    let room_resp = server
        .app()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/rooms/{conv_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(room_resp.status(), StatusCode::OK);
    let room_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(room_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(room_json["room_type"], "direct");
}

#[tokio::test]
async fn test_conversation_send_message_uses_room_pipeline() {
    let server = make_server().await;

    let agent_body = serde_json::json!({
        "name": "conv-pipeline-agent",
        "model": "stub",
        "system_prompt": "You are helpful."
    });
    let agent_resp = server
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agents")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&agent_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let agent_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(agent_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let agent_id = agent_json["id"].as_str().unwrap();

    let conv_body = serde_json::json!({ "agent_id": agent_id });
    let conv_resp = server
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/conversations")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&conv_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let conv_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(conv_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let conv_id = conv_json["id"].as_str().unwrap();

    let msg_body = serde_json::json!({ "content": "Hello via conversation API" });
    let msg_resp = server
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/conversations/{conv_id}/messages"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&msg_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(msg_resp.status(), StatusCode::ACCEPTED);
    let msg_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(msg_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(msg_json["status"], "turn_queued");
    assert!(msg_json["room_message"]["seq"].as_u64().unwrap_or(0) >= 1);

    let list_resp = server
        .app()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/conversations/{conv_id}/messages"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_resp.status(), StatusCode::OK);
    let list_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(list_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(list_json["total"], 1);
    assert_eq!(list_json["room_id"], conv_id);
}

/// Register then login on the same in-memory server instance.
///
/// Postgres fallback (`find_user_by_email`) handles gateway restarts when users
/// exist in the database but the in-memory cache is cold — that path requires
/// a configured `DATABASE_URL` and is not exercised here.
#[tokio::test]
async fn test_auth_register_then_login() {
    let server = make_server().await;
    let email = format!("user-{}@test.local", uuid::Uuid::new_v4());
    let password = "securepass123";

    let register_body = serde_json::json!({
        "email": email,
        "password": password,
    });
    let reg_resp = server
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/system/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&register_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reg_resp.status(), StatusCode::CREATED);

    let login_body = serde_json::json!({
        "email": email,
        "password": password,
    });
    let login_resp = server
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/system/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&login_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_resp.status(), StatusCode::OK);
    let login_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(login_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(login_json["token"].as_str().is_some());
    assert_eq!(login_json["email"], email);
}

#[tokio::test]
async fn test_create_channel_roundtrip() {
    let server = make_server().await;
    let body = serde_json::json!({
        "name": "support-webhook",
        "channel_type": "webhook",
        "config": { "url": "https://example.com/hook" }
    });
    let created = server
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/channels")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let channel: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let channel_id = channel["id"].as_str().unwrap();

    let fetched = server
        .app()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/channels/{channel_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetched.status(), StatusCode::OK);
}
