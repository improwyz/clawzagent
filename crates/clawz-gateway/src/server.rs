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

use axum::{routing::get, Router};
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;
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
        // Use a permissive CORS layer for local development and SPA front-ends.
        // In production this should be tightened to the exact allowed origins.
        let cors = CorsLayer::permissive();

        Router::new()
            .route("/health", get(health))
            .route("/api/docs", get(swagger_ui))
            .nest("/api/v1", Self::api_routes())
            .nest("/ws", Self::ws_routes())
            .layer(TraceLayer::new_for_http())
            .layer(cors)
            .with_state(state)
    }

    /// Assemble the `/api/v1/*` REST route tree.
    ///
    /// Each nested path delegates to a dedicated sub-module under [`crate::routes`].
    fn api_routes() -> Router<AppState> {
        Router::new()
            .nest("/agents", crate::routes::agents::routes())
            .nest("/conversations", crate::routes::conversations::routes())
            .nest("/channels", crate::routes::channels::routes())
            .nest("/providers", crate::routes::providers::routes())
            .nest("/tools", crate::routes::tools::routes())
            .nest("/governance", crate::routes::governance::routes())
            .nest("/fleet", crate::routes::fleet::routes())
            .nest("/system", crate::routes::system::routes())
    }

    /// Assemble the `/ws/*` WebSocket upgrade route tree.
    ///
    /// These endpoints upgrade HTTP connections to WebSocket for real-time
    /// streaming of agent output, system events, metrics, approval queues, logs,
    /// and voice channel data.
    fn ws_routes() -> Router<AppState> {
        Router::new()
            .route("/agent/{id}/stream", get(crate::ws::handlers::agent_stream))
            .route(
                "/agents/{id}/stream",
                get(crate::ws::handlers::autonomous_stream),
            )
            .route("/events", get(crate::ws::handlers::events))
            .route("/metrics", get(crate::ws::handlers::metrics))
            .route("/approvals", get(crate::ws::handlers::approvals))
            .route("/logs", get(crate::ws::handlers::logs))
            .route("/voice", get(crate::ws::handlers::voice))
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
    "Swagger UI placeholder"
}
