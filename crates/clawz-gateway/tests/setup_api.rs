use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use clawz_gateway::{AppState, server::GatewayServer};
use http_body_util::BodyExt;
use std::fs;
use std::sync::Mutex;
use tower::ServiceExt;
use uuid::Uuid;

static TEST_ENV: Mutex<()> = Mutex::new(());

fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_ENV
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct TestServer {
    _env_guard: std::sync::MutexGuard<'static, ()>,
    _home: std::path::PathBuf,
    app: axum::Router,
}

async fn make_setup_server() -> TestServer {
    let guard = test_env_lock();
    let home = std::env::temp_dir().join(format!("clawz-setup-test-{}", Uuid::new_v4()));
    fs::create_dir_all(&home).expect("create CLAWZ_HOME");
    unsafe {
        std::env::set_var("CLAWZ_HOME", &home);
        std::env::set_var("CLAWZ_DISABLE_AUTH", "1");
    }
    let state = AppState::new("test-secret-key");
    let server = GatewayServer::new(state);
    TestServer {
        _env_guard: guard,
        _home: home,
        app: server.app,
    }
}

#[tokio::test]
async fn setup_status_returns_incomplete_with_temp_clawz_home() {
    let server = make_setup_server().await;
    let response = server
        .app
        .oneshot(
            Request::builder()
                .uri("/api/v1/setup/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["setup_complete"], false);
    assert!(json["step"].is_string());
    assert!(json["bootstrap_token"].is_string());
    assert!(!json["bootstrap_token"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn setup_oauth_start_anthropic_returns_real_authorize_url() {
    let server = make_setup_server().await;
    let response = server
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/setup/oauth/start")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"provider":"anthropic"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let auth_url = json["auth_url"].as_str().expect("auth_url");
    assert!(auth_url.starts_with("https://claude.ai/oauth/authorize"));
    assert!(auth_url.contains("code_challenge="));
    assert!(auth_url.contains("client_id=9d1c250a"));
}

#[tokio::test]
async fn setup_oauth_start_skip_has_no_auth_url() {
    let server = make_setup_server().await;
    let response = server
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/setup/oauth/start")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"provider":"skip"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["auth_url"].is_null());
}
