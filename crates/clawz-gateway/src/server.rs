//! HTTP server setup for the Clawz Gateway.
//!
//! This module wires the Axum [`Router`] with all REST and WebSocket routes,
//! applies Tower middleware (CORS, request tracing), and binds to a TCP socket.
//!
//! Responsibilities:
//! - Compose the top-level route tree from sub-route modules.
//! - Attach shared [`AppState`](crate::AppState) to every handler via [`axum::extract::State`].
//! - Provide a [`GatewayServer::serve`] entry point that blocks until the server shuts down.
//!
//! # Cross-module dependencies
//! - [`crate::routes`] — REST API handlers for agents, conversations, fleet, etc.
//! - [`crate::ws`] — WebSocket upgrade handlers for streaming and real-time events.
//! - [`crate::AppState`] — Shared in-memory state and broadcast channel.

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use std::net::SocketAddr;
use std::path::Path;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

// Dependency: AppState is defined in the crate root and shared across all routes.
use crate::AppState;

/// Axum-based API gateway server.
///
/// Holds the fully-constructed [`Router`] so that callers can either serve it
/// directly or mount it as a nested router inside a larger application.
#[derive(Clone)]
pub struct GatewayServer {
    /// The composed Axum application including all routes, state, and middleware.
    pub app: Router,
}

impl GatewayServer {
    /// Build a new gateway server with all routes, state, and middleware.
    ///
    /// # Arguments
    /// - `state` — the shared [`AppState`] containing registries, broadcast channel, and secrets.
    ///
    /// # Example
    /// ```ignore
    /// // Full example — run with: cargo run --bin clawz-gateway
    /// let state = AppState::default();
    /// let server = GatewayServer::new(state);
    /// server.serve("0.0.0.0:3000".parse().unwrap()).await?;
    /// ```
    pub fn new(state: AppState) -> Self {
        let app = Self::build_router(state);
        Self { app }
    }

    /// Construct the Axum [`Router`] by nesting sub-routers and layering middleware.
    ///
    /// The stack is built bottom-up:
    /// 1. Register individual API and WebSocket routes.
    /// 2. Nest them under path prefixes (`/api/v1`, `/ws`).
    /// 3. Add global middleware (request tracing, CORS).
    /// 4. Attach [`AppState`] so every handler can extract it.
    pub fn build_router(state: AppState) -> Router {
        // Fail-closed: in release builds, refuse to start without the secrets that
        // protect auth and stored credentials. No-op in debug for local dev.
        validate_release_config();

        // CORS is restricted via `CLAWZ_CORS_ALLOWED_ORIGINS`; see [`build_cors`].
        let cors = build_cors();

        let webhooks = Router::new()
            .merge(crate::routes::telephony::routes())
            .merge(crate::routes::webhooks::routes())
            .layer(middleware::from_fn(crate::ratelimit::rate_limit));

        let mut router = Router::new()
            .route("/health", get(health))
            .route("/api/docs", get(swagger_ui))
            .nest("/webhooks", webhooks)
            .nest("/api/v1", Self::api_routes())
            .nest("/ws", crate::ws::ws_routes());

        if let Some(dist) = Self::web_dist_path() {
            tracing::info!("serving dashboard from {}", dist.display());
            router = router.fallback_service(
                ServeDir::new(&dist).not_found_service(ServeFile::new(dist.join("index.html"))),
            );
        }

        router
            .layer(middleware::from_fn(security_headers))
            // Idempotency-Key replay for mutating requests (no-op unless a DB
            // pool is registered and the request carries the header).
            .layer(middleware::from_fn(crate::idempotency::idempotency))
            .layer(TraceLayer::new_for_http())
            .layer(cors)
            // Cap inbound request bodies to bound memory use / reject oversized
            // payloads. Only affects request bodies (not streaming responses or
            // WS, which carry no upgrade-request body). Tunable via
            // CLAWZ_MAX_BODY_BYTES; defaults to axum's historical 2 MiB.
            .layer(axum::extract::DefaultBodyLimit::max(max_body_bytes()))
            .with_state(state)
    }

    /// Resolve SPA static files from `CLAWZ_WEB_DIST` or `web/dist` when present.
    fn web_dist_path() -> Option<std::path::PathBuf> {
        if let Ok(dist) = std::env::var("CLAWZ_WEB_DIST") {
            let p = Path::new(&dist);
            if p.join("index.html").exists() {
                return Some(p.to_path_buf());
            }
        }
        let rel = Path::new("web/dist");
        if rel.join("index.html").exists() {
            return Some(rel.to_path_buf());
        }
        None
    }

    /// Assemble the `/api/v1/*` REST route tree.
    ///
    /// Each nested path delegates to a dedicated sub-module under [`crate::routes`].
    fn api_routes() -> Router<AppState> {
        Router::new()
            .nest("/agents", crate::routes::agents::routes())
            .nest("/conversations", crate::routes::conversations::routes())
            .nest("/sessions", crate::routes::sessions::routes())
            .nest("/skills", crate::routes::skills::routes())
            .nest("/rooms", crate::routes::rooms::routes())
            .nest("/channels", crate::routes::channels::routes())
            .nest("/cron", crate::routes::cron::routes())
            .nest("/background", crate::routes::background::routes())
            .nest("/providers", crate::routes::providers::routes())
            .nest("/tools", crate::routes::tools::routes())
            .nest("/governance", crate::routes::governance::routes())
            .nest("/fleet", crate::routes::fleet::routes())
            .nest("/cloud", crate::routes::cloud_deploy::routes())
            .nest("/system", crate::routes::system::routes())
            .nest("/setup", crate::routes::setup::routes())
            .nest("/dashboard", crate::routes::dashboard::routes())
            .route("/mcp", post(crate::mcp::handle_mcp_request))
            .layer(middleware::from_fn(crate::auth::auth_middleware))
            // Outermost: reject floods before auth/handlers run.
            .layer(middleware::from_fn(crate::ratelimit::rate_limit))
    }

    /// Bind and serve the gateway on the given address.
    ///
    /// This function blocks the current task until the server exits (either
    /// because the [`tokio::net::TcpListener`] closes or the process receives
    /// a shutdown signal). Callers should coordinate shutdown via
    /// [`crate::shutdown::ShutdownCoordinator`].
    ///
    /// # Errors
    /// Returns an error if the TCP listener cannot bind to `addr` or if the
    /// Axum serve loop encounters an unexpected I/O error.
    pub async fn serve(self, addr: SocketAddr) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, self.app).await?;
        Ok(())
    }
}

impl Default for GatewayServer {
    /// Create a [`GatewayServer`] backed by [`AppState::default()`].
    ///
    /// Useful for integration tests and quick local runs.
    fn default() -> Self {
        Self::new(AppState::default())
    }
}

/// Simple liveness probe used by load balancers and container orchestrators.
///
/// Returns `"OK"` with HTTP 200. This endpoint intentionally performs no
/// dependency health checks in order to stay lightweight; deeper readiness
/// probes can be added under `/health/ready` in the future.
async fn health() -> &'static str {
    "OK"
}

/// Placeholder for an OpenAPI/Swagger UI documentation endpoint.
///
/// In production this should serve the HTML bundle generated from an OpenAPI
/// spec (e.g., via `utoipa` or `rapidoc`). For now it returns plain text so
/// that the `/api/docs` route does not 404 during early development.
async fn swagger_ui() -> impl axum::response::IntoResponse {
    axum::response::Redirect::temporary("/api/v1/system/openapi")
}

/// In release builds, refuse to start without production secrets. No-op in debug.
///
/// Enforces fail-closed deployment: a misconfigured production gateway aborts at
/// startup with a clear message instead of silently running with the well-known
/// dev fallbacks (`"changeme"` JWT secret, `dev-insecure-key` secrets key) or an
/// auth bypass left enabled.
fn validate_release_config() {
    #[cfg(not(debug_assertions))]
    {
        let jwt_set =
            std::env::var("CLAWZ_JWT_SECRET").is_ok() || std::env::var("JWT_SECRET").is_ok();
        assert!(
            jwt_set,
            "CLAWZ_JWT_SECRET (or JWT_SECRET) must be set in release builds"
        );
        assert!(
            std::env::var("CLAWZ_SECRETS_KEY").is_ok(),
            "CLAWZ_SECRETS_KEY must be set in release builds"
        );
        assert!(
            std::env::var("CLAWZ_DISABLE_AUTH").as_deref() != Ok("1"),
            "CLAWZ_DISABLE_AUTH must not be enabled in release builds"
        );
    }
}

/// Maximum inbound request body size in bytes.
///
/// Reads `CLAWZ_MAX_BODY_BYTES`; falls back to 2 MiB (axum's historical default)
/// so behavior is preserved unless an operator opts into a different cap.
fn max_body_bytes() -> usize {
    const DEFAULT: usize = 2 * 1024 * 1024;
    std::env::var("CLAWZ_MAX_BODY_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT)
}

/// Build the CORS layer from `CLAWZ_CORS_ALLOWED_ORIGINS` (comma-separated origins).
///
/// - When set, only those origins are allowed.
/// - When unset in debug builds, a permissive layer is used for local development.
/// - When unset in release builds, no cross-origin grants are issued, so browsers
///   fall back to same-origin only (secure default).
fn build_cors() -> CorsLayer {
    if let Ok(raw) = std::env::var("CLAWZ_CORS_ALLOWED_ORIGINS") {
        let origins: Vec<HeaderValue> = raw
            .split(',')
            .filter_map(|o| o.trim().parse().ok())
            .collect();
        if !origins.is_empty() {
            return CorsLayer::new()
                .allow_origin(origins)
                .allow_methods(Any)
                .allow_headers(Any);
        }
    }

    #[cfg(debug_assertions)]
    {
        CorsLayer::permissive()
    }
    #[cfg(not(debug_assertions))]
    {
        CorsLayer::new()
    }
}

/// Attach baseline security response headers to every response.
///
/// HSTS is only emitted in release builds to avoid pinning HTTPS during local HTTP
/// development. A Content-Security-Policy is opt-in via `CLAWZ_CSP` so the bundled
/// SPA is not broken by a default policy.
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    #[cfg(not(debug_assertions))]
    headers.insert(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    if let Ok(csp) = std::env::var("CLAWZ_CSP") {
        if let Ok(value) = HeaderValue::from_str(&csp) {
            headers.insert(header::CONTENT_SECURITY_POLICY, value);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request as HttpRequest, routing::get};
    use tower::ServiceExt;

    #[tokio::test]
    async fn security_headers_are_set_on_responses() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(middleware::from_fn(security_headers));
        let res = app
            .oneshot(HttpRequest::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let h = res.headers();
        assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(h.get("x-frame-options").unwrap(), "DENY");
        assert_eq!(h.get("referrer-policy").unwrap(), "no-referrer");
    }

    #[test]
    fn build_cors_does_not_panic() {
        let _ = build_cors();
    }
}
