//! Install onboarding wizard REST API (`/api/v1/setup/*`).
//!
//! Public routes (no JWT) — mutating handlers require `X-Clawz-Setup-Token`
//! matching `CLAWZ_SETUP_BOOTSTRAP_TOKEN` or `~/.clawz/setup/bootstrap.token`.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use clawz_setup::{
    AgentBootstrap, ClawzUserConfig, ConfirmGate, DeploymentChoice, HostExecPolicy,
    HostSpecChecker, IdentityInput, InstallStrategy, OAuthStartResult, SetupOAuthProvider,
    SetupPlatform, SetupStateMachine, SetupStep, SetupToolRegistry, ToolContext, ToolInput,
    ensure_setup_dir, init_workspace_at, load_session, oauth_complete,
    oauth_start as setup_oauth_start, resolve_repo_root, save_session, session_path, setup_dir,
    workspace_root,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use crate::{AgentRecord, AgentStatus, AppState, GatewayError, ProviderRecord};

const SETUP_TOKEN_HEADER: &str = "x-clawz-setup-token";

/// Assemble the setup sub-router (`/api/v1/setup`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/status", get(status))
        .route("/session", post(start_session))
        .route("/answer", post(answer))
        .route("/apply", post(apply))
        .route("/complete", post(complete))
        .route("/oauth/start", post(oauth_start))
        .route(
            "/oauth/callback",
            get(oauth_callback_browser).post(oauth_callback),
        )
        .route("/stack/status", get(stack_status))
        .route("/stack", post(stack_mutate))
}

// ─── Request / response types ─────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SetupStatusResponse {
    pub setup_complete: bool,
    pub step: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Bootstrap secret for `X-Clawz-Setup-Token` on mutating routes (omitted when setup is complete).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootstrap_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SessionBody {
    pub platform: Option<String>,
    #[serde(default)]
    pub resume: bool,
}

#[derive(Debug, Deserialize)]
pub struct AnswerBody {
    pub deployment: Option<String>,
    pub install_strategy: Option<String>,
    #[serde(default)]
    pub secrets: HashMap<String, String>,
    pub llm_provider: Option<String>,
    pub llm_api_key: Option<String>,
    pub identity_name: Option<String>,
    pub identity_who_am_i: Option<String>,
    pub identity_role: Option<String>,
    pub identity_model: Option<String>,
    #[serde(default)]
    pub advance: bool,
}

#[derive(Debug, Deserialize)]
pub struct ApplyBody {
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Deserialize)]
pub struct OAuthStartBody {
    pub provider: String,
}

pub type OAuthStartResponse = OAuthStartResult;

#[derive(Debug, Deserialize)]
pub struct OAuthCallbackBody {
    pub state: String,
    pub code: Option<String>,
    pub token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StackBody {
    /// `deps`, `up`, `down`, or `migrate`.
    pub action: String,
    pub install_strategy: Option<String>,
    #[serde(default)]
    pub with_web: bool,
    #[serde(default)]
    pub dry_run: bool,
    /// Required for mutating tools when confirm gate is enforced (e.g. `yes-install`).
    pub confirm: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StackStatusResponse {
    pub host_exec_allowed: bool,
    pub suggested_command: &'static str,
    pub docker_available: bool,
    pub compose_v2_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
}

// ─── Bootstrap token ──────────────────────────────────────────────────────────

fn bootstrap_token_path() -> PathBuf {
    setup_dir().join("bootstrap.token")
}

fn load_or_create_bootstrap_token() -> Result<String, GatewayError> {
    if let Ok(token) = std::env::var("CLAWZ_SETUP_BOOTSTRAP_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let path = bootstrap_token_path();
    if path.exists() {
        let data = fs::read_to_string(&path)
            .map_err(|e| GatewayError::Internal(format!("read bootstrap token: {e}")))?;
        let token = data.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    ensure_setup_dir().map_err(map_setup_error)?;
    let token = Uuid::new_v4().to_string();
    fs::write(&path, &token)
        .map_err(|e| GatewayError::Internal(format!("write bootstrap token: {e}")))?;
    Ok(token)
}

fn verify_bootstrap_token(headers: &HeaderMap) -> Result<(), GatewayError> {
    let presented = headers
        .get(SETUP_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok());
    let expected = load_or_create_bootstrap_token()?;
    match presented {
        Some(t) if t == expected => Ok(()),
        _ => Err(GatewayError::Unauthorized(
            "missing or invalid X-Clawz-Setup-Token".into(),
        )),
    }
}

fn setup_complete() -> bool {
    ClawzUserConfig::load()
        .map(|c| c.setup_complete)
        .unwrap_or(false)
}

fn guard_mutations_allowed() -> Result<(), GatewayError> {
    if setup_complete() {
        return Err(GatewayError::Conflict(
            "setup is already complete; bootstrap routes are disabled".into(),
        ));
    }
    Ok(())
}

fn map_setup_error(err: clawz_setup::SetupError) -> GatewayError {
    match err {
        clawz_setup::SetupError::Aborted => {
            GatewayError::Conflict("setup session was aborted".into())
        }
        clawz_setup::SetupError::DeployModeRequired { step } => {
            GatewayError::Unprocessable(format!("deployment mode required before {step}"))
        }
        clawz_setup::SetupError::InvalidTransition(msg) => {
            GatewayError::Unprocessable(format!("invalid transition: {msg}"))
        }
        clawz_setup::SetupError::SessionNotFound { path } => GatewayError::NotFound {
            resource: "setup_session".into(),
            id: path,
        },
        clawz_setup::SetupError::Internal(msg) => GatewayError::Internal(msg),
        other => GatewayError::Internal(other.to_string()),
    }
}

fn parse_platform(raw: Option<&str>) -> SetupPlatform {
    match raw.unwrap_or("unknown").to_ascii_lowercase().as_str() {
        "linux" => SetupPlatform::Linux,
        "macos" | "mac" | "darwin" => SetupPlatform::MacOs,
        "windows" | "win" => SetupPlatform::Windows,
        "web" => SetupPlatform::Web,
        "mobile" => SetupPlatform::Mobile,
        _ => SetupPlatform::Unknown,
    }
}

fn parse_deployment(raw: &str) -> Result<DeploymentChoice, GatewayError> {
    match raw.to_ascii_lowercase().as_str() {
        "standalone" => Ok(DeploymentChoice::Standalone),
        "micro" => Ok(DeploymentChoice::Micro),
        "elastic" => Ok(DeploymentChoice::Elastic),
        other => Err(GatewayError::Unprocessable(format!(
            "unknown deployment mode: {other}"
        ))),
    }
}

fn parse_install_strategy(raw: &str) -> Result<InstallStrategy, GatewayError> {
    match raw.to_ascii_lowercase().as_str() {
        "prebuilt" => Ok(InstallStrategy::Prebuilt),
        "build" => Ok(InstallStrategy::Build),
        "source" => Ok(InstallStrategy::Source),
        other => Err(GatewayError::Unprocessable(format!(
            "unknown install strategy: {other}"
        ))),
    }
}

fn step_name(step: SetupStep) -> String {
    serde_json::to_value(step)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{step:?}"))
}

fn load_machine() -> Result<SetupStateMachine, GatewayError> {
    SetupStateMachine::resume(SetupPlatform::Unknown)
        .map_err(map_setup_error)?
        .ok_or_else(|| GatewayError::NotFound {
            resource: "setup_session".into(),
            id: session_path().display().to_string(),
        })
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /setup/status` — wizard progress and bootstrap token provisioning.
async fn status() -> Result<Json<SetupStatusResponse>, GatewayError> {
    let bootstrap = load_or_create_bootstrap_token()?;

    let user = ClawzUserConfig::load().map_err(map_setup_error)?;
    let (step, session_id) = match load_session() {
        Ok(session) => (
            step_name(session.current_step),
            Some(session.id.to_string()),
        ),
        Err(clawz_setup::SetupError::SessionNotFound { .. }) => {
            (step_name(SetupStep::Welcome), None)
        }
        Err(e) => return Err(map_setup_error(e)),
    };

    let bootstrap_token = if user.setup_complete {
        None
    } else {
        Some(bootstrap)
    };

    Ok(Json(SetupStatusResponse {
        setup_complete: user.setup_complete,
        step,
        session_id,
        bootstrap_token,
    }))
}

/// `POST /setup/session` — start or resume the setup state machine.
async fn start_session(Json(body): Json<SessionBody>) -> Result<Json<Value>, GatewayError> {
    guard_mutations_allowed()?;

    let platform = parse_platform(body.platform.as_deref());
    let machine = if body.resume {
        SetupStateMachine::resume(platform)
            .map_err(map_setup_error)?
            .unwrap_or_else(|| SetupStateMachine::new(platform))
    } else {
        SetupStateMachine::new(platform)
    };

    save_session(machine.session()).map_err(map_setup_error)?;

    Ok(Json(json!({
        "session_id": machine.session().id,
        "step": step_name(machine.current_step()),
        "platform": body.platform,
        "resumed": body.resume,
    })))
}

/// `POST /setup/answer` — record structured wizard answers and advance steps.
async fn answer(Json(body): Json<AnswerBody>) -> Result<Json<Value>, GatewayError> {
    guard_mutations_allowed()?;

    let mut machine = load_machine()?;
    let mut advanced_to: Option<String> = None;

    if let Some(ref dep) = body.deployment {
        let choice = parse_deployment(dep)?;
        machine.set_deployment(choice).map_err(map_setup_error)?;
    }

    if let Some(ref strat) = body.install_strategy {
        let strategy = parse_install_strategy(strat)?;
        machine
            .set_install_strategy(strategy)
            .map_err(map_setup_error)?;
    }

    for (field, value) in &body.secrets {
        record_user_answer(&mut machine, SetupStep::WriteSecrets, field, value)?;
    }

    if let Some(ref provider) = body.llm_provider {
        record_user_answer(&mut machine, SetupStep::Llm, "provider", provider)?;
        if let Some(ref key) = body.llm_api_key {
            record_user_answer(&mut machine, SetupStep::Llm, "api_key", key)?;
        }
    } else if body.llm_api_key.is_some() {
        return Err(GatewayError::Unprocessable(
            "llm_provider is required when llm_api_key is set".into(),
        ));
    }

    if body.identity_name.is_some()
        || body.identity_who_am_i.is_some()
        || body.identity_role.is_some()
    {
        let name = body.identity_name.clone().unwrap_or_else(|| "ClawZ".into());
        let who = body
            .identity_who_am_i
            .clone()
            .unwrap_or_else(|| "a helpful ClawZ assistant".into());
        let role = body
            .identity_role
            .clone()
            .unwrap_or_else(|| "general-purpose agent".into());
        record_user_answer(&mut machine, SetupStep::AgentIdentity, "name", &name)?;
        record_user_answer(&mut machine, SetupStep::AgentIdentity, "who_am_i", &who)?;
        record_user_answer(&mut machine, SetupStep::AgentIdentity, "role", &role)?;
        if let Some(ref model) = body.identity_model {
            record_user_answer(&mut machine, SetupStep::AgentIdentity, "model", model)?;
        }
    }

    if body.advance {
        let step = machine.advance().map_err(map_setup_error)?;
        advanced_to = Some(step_name(step));
    }

    save_session(machine.session()).map_err(map_setup_error)?;

    Ok(Json(json!({
        "session_id": machine.session().id,
        "step": step_name(machine.current_step()),
        "advanced_to": advanced_to,
    })))
}

fn record_user_answer(
    machine: &mut SetupStateMachine,
    step: SetupStep,
    field: &str,
    value: &str,
) -> Result<(), GatewayError> {
    machine
        .session_mut()
        .events
        .push(clawz_setup::SetupEvent::UserAnswer {
            step,
            field: field.to_string(),
            value: value.to_string(),
        });
    machine.session_mut().touch();
    Ok(())
}

/// `POST /setup/apply` — promote session answers into gateway state and workspace.
async fn apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ApplyBody>,
) -> Result<Json<Value>, GatewayError> {
    guard_mutations_allowed()?;
    verify_bootstrap_token(&headers)?;

    if setup_complete() && !body.force {
        return Err(GatewayError::Conflict("setup already complete".into()));
    }

    let machine = load_machine()?;
    let session = machine.session();

    let provider_type =
        find_answer(session, SetupStep::Llm, "provider").unwrap_or_else(|| "anthropic".into());
    let api_key = find_answer(session, SetupStep::Llm, "api_key");

    let now = Utc::now();
    let provider_id = Uuid::new_v4().to_string();
    let provider = ProviderRecord {
        id: provider_id.clone(),
        name: format!("Setup {provider_type}"),
        provider_type: provider_type.clone(),
        api_key: api_key.clone(),
        base_url: None,
        enabled: true,
        created_at: now,
        updated_at: now,
    };
    state.providers.write().await.push(provider.clone());

    let identity = build_identity_from_session(session);
    let artifacts = AgentBootstrap::build_identity(&identity);
    let agent_payload = AgentBootstrap::agent_config(&identity, &artifacts);

    let agent = AgentRecord {
        id: agent_payload.id.to_string(),
        name: agent_payload.name.clone(),
        model: agent_payload.model.clone(),
        description: Some("Created by setup wizard".into()),
        system_prompt: Some(agent_payload.system_prompt.clone()),
        status: AgentStatus::Idle,
        created_at: now,
        updated_at: now,
    };
    state.agents.write().await.push(agent.clone());
    state.persist_agent_record(&agent).await;

    let workspace = workspace_root();
    init_workspace_at(&workspace).map_err(map_setup_error)?;
    let agents_md_path = workspace.join("AGENTS.md");
    fs::write(&agents_md_path, &artifacts.agents_md)
        .map_err(|e| GatewayError::Internal(format!("write AGENTS.md: {e}")))?;

    state
        .append_audit(
            "system",
            "setup.apply",
            "setup",
            &session.id.to_string(),
            Some(
                json!({
                    "provider_id": provider_id,
                    "agent_id": agent.id,
                    "workspace": workspace.display().to_string(),
                })
                .to_string(),
            ),
        )
        .await;

    Ok(Json(json!({
        "applied": true,
        "provider_id": provider_id,
        "agent_id": agent.id,
        "workspace": workspace,
    })))
}

/// `POST /setup/complete` — finalize wizard and persist `setup_complete`.
async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    guard_mutations_allowed()?;
    verify_bootstrap_token(&headers)?;

    let mut machine = load_machine()?;
    let session_id = machine.session().id.to_string();
    machine.complete().map_err(map_setup_error)?;

    state
        .append_audit("system", "setup.complete", "setup", &session_id, None)
        .await;

    Ok((
        StatusCode::OK,
        Json(json!({
            "setup_complete": true,
            "session_id": session_id,
            "step": step_name(SetupStep::Complete),
        })),
    ))
}

/// `POST /setup/oauth/start` — real OAuth authorize URLs (PKCE) or Cursor import.
async fn oauth_start(
    Json(body): Json<OAuthStartBody>,
) -> Result<Json<OAuthStartResponse>, GatewayError> {
    guard_mutations_allowed()?;

    let provider = SetupOAuthProvider::parse(&body.provider).ok_or_else(|| {
        GatewayError::Unprocessable(format!("unsupported oauth provider: {}", body.provider))
    })?;

    let result = setup_oauth_start(provider).map_err(map_setup_error)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct OAuthCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// `GET /setup/oauth/callback` — browser redirect target after provider sign-in.
async fn oauth_callback_browser(
    State(state): State<AppState>,
    Query(query): Query<OAuthCallbackQuery>,
) -> Result<Response, GatewayError> {
    guard_mutations_allowed()?;

    if let Some(err) = query.error {
        let detail = query.error_description.unwrap_or(err);
        return Ok(Html(format!(
            r#"<!DOCTYPE html><html><body style="font-family:system-ui;background:#18181b;color:#fafafa;padding:2rem">
            <h1>Setup sign-in failed</h1><p>{detail}</p>
            <p>You can close this tab and return to the setup wizard.</p></body></html>"#
        ))
        .into_response());
    }

    let state_id = query
        .state
        .ok_or_else(|| GatewayError::Unprocessable("missing state".into()))?;
    let code = query
        .code
        .ok_or_else(|| GatewayError::Unprocessable("missing code".into()))?;

    let bundle = oauth_complete(&state_id, Some(&code), None).map_err(map_setup_error)?;
    sync_vault_from_bundle(&state, &bundle).await;

    let setup_url =
        std::env::var("CLAWZ_SETUP_UI_URL").unwrap_or_else(|_| "/setup?oauth=success".into());

    Ok(Html(format!(
        r#"<!DOCTYPE html><html><head>
        <meta http-equiv="refresh" content="2;url={setup_url}">
        </head><body style="font-family:system-ui;background:#18181b;color:#fafafa;padding:2rem">
        <h1>Signed in successfully</h1>
        <p>Provider: {}. Redirecting back to setup…</p>
        <p><a href="{setup_url}" style="color:#60a5fa">Continue setup</a></p>
        </body></html>"#,
        bundle.provider
    ))
    .into_response())
}

/// `POST /setup/oauth/callback` — exchange code or store API key token.
async fn oauth_callback(
    State(state): State<AppState>,
    Json(body): Json<OAuthCallbackBody>,
) -> Result<Json<Value>, GatewayError> {
    guard_mutations_allowed()?;

    let bundle = oauth_complete(&body.state, body.code.as_deref(), body.token.as_deref())
        .map_err(map_setup_error)?;

    sync_vault_from_bundle(&state, &bundle).await;

    Ok(Json(json!({
        "stored": true,
        "state": body.state,
        "provider": bundle.provider,
        "source": bundle.source,
    })))
}

/// `GET /setup/stack/status` — host exec policy and Docker readiness (read-only).
async fn stack_status() -> Result<Json<StackStatusResponse>, GatewayError> {
    let spec = HostSpecChecker::collect();
    let repo_root = resolve_repo_root().ok().map(|p| p.display().to_string());
    Ok(Json(StackStatusResponse {
        host_exec_allowed: HostExecPolicy::allowed(),
        suggested_command: HostExecPolicy::suggested_install_command(),
        docker_available: spec.docker_available,
        compose_v2_available: spec.compose_v2_available,
        repo_root,
    }))
}

/// `POST /setup/stack` — install deps or start/stop Compose stack on the host.
async fn stack_mutate(
    headers: HeaderMap,
    Json(body): Json<StackBody>,
) -> Result<Json<Value>, GatewayError> {
    guard_mutations_allowed()?;
    verify_bootstrap_token(&headers)?;

    let host_exec_allowed = HostExecPolicy::allowed();
    let suggested = HostExecPolicy::suggested_install_command();

    if !host_exec_allowed && !body.dry_run {
        return Ok(Json(json!({
            "ok": false,
            "host_exec_allowed": false,
            "suggested_command": suggested,
            "message": "host bootstrap is disabled inside the gateway container; run the suggested command on the host",
        })));
    }

    let repo_root = resolve_repo_root().map_err(map_setup_error)?;
    let machine = load_machine()?;
    let mut registry = SetupToolRegistry::with_default_tools();
    if let Some(ref confirm) = body.confirm {
        registry.set_confirm_gate(ConfirmGate::new(confirm.clone()));
    }

    let ctx = ToolContext {
        session: machine.session().clone(),
        confirm_token: body.confirm.clone(),
        repo_root: repo_root.clone(),
        env_path: repo_root.join(".env"),
    };

    let tool_args = json!({
        "dry_run": body.dry_run,
        "with_web": body.with_web,
        "install_strategy": body.install_strategy,
    });
    let input = ToolInput { args: tool_args };

    let tool_name = match body.action.as_str() {
        "deps" => "install_deps",
        "up" => "compose_up",
        "down" => "compose_down",
        "migrate" => "migrate_db",
        other => {
            return Err(GatewayError::Unprocessable(format!(
                "unknown stack action: {other} (use deps, up, down, migrate)"
            )));
        }
    };

    let result = registry
        .run(tool_name, &ctx, &input)
        .map_err(map_setup_error)?;

    Ok(Json(json!({
        "ok": result.ok,
        "host_exec_allowed": host_exec_allowed,
        "suggested_command": if host_exec_allowed { Value::Null } else { json!(suggested) },
        "tool": tool_name,
        "dry_run": body.dry_run,
        "message": result.message,
    })))
}

async fn sync_vault_from_bundle(state: &AppState, bundle: &clawz_setup::OAuthTokenBundle) {
    let mut vault = state.setup_oauth_vault.write().await;
    vault.insert(
        bundle.provider.clone(),
        serde_json::to_string(bundle).unwrap_or_else(|_| bundle.access_token.clone()),
    );
}

fn find_answer(
    session: &clawz_setup::SetupSession,
    step: SetupStep,
    field: &str,
) -> Option<String> {
    session.events.iter().rev().find_map(|ev| match ev {
        clawz_setup::SetupEvent::UserAnswer {
            step: s,
            field: f,
            value,
        } if *s == step && f == field => Some(value.clone()),
        _ => None,
    })
}

fn build_identity_from_session(session: &clawz_setup::SetupSession) -> IdentityInput {
    IdentityInput {
        name: find_answer(session, SetupStep::AgentIdentity, "name")
            .unwrap_or_else(|| "ClawZ".into()),
        who_am_i: find_answer(session, SetupStep::AgentIdentity, "who_am_i")
            .unwrap_or_else(|| "a helpful ClawZ assistant".into()),
        role: find_answer(session, SetupStep::AgentIdentity, "role")
            .unwrap_or_else(|| "general-purpose agent".into()),
        model: find_answer(session, SetupStep::AgentIdentity, "model"),
    }
}
