//! Workspace skills API — lists `skills/*/SKILL.md` from the operator workspace.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AppState, GatewayError};

/// Mounted at `/api/v1/skills`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_skills).post(create_skill))
        .route("/{name}", get(get_skill))
}

fn loader() -> clawz_worker::workspace::WorkspaceLoader {
    clawz_worker::workspace::WorkspaceLoader::default_home()
}

async fn list_skills(
    State(_state): State<AppState>,
) -> Result<Json<Value>, GatewayError> {
    let snap = loader()
        .load_snapshot()
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    let data: Vec<Value> = snap
        .skills
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "path": s.path.to_string_lossy(),
            })
        })
        .collect();

    Ok(Json(json!({
        "root": snap.root.to_string_lossy(),
        "data": data,
        "total": data.len(),
        "has_agents_md": snap.agents_md.is_some(),
        "has_soul_md": snap.soul_md.is_some(),
    })))
}

async fn get_skill(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let snap = loader()
        .load_snapshot()
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    let skill = snap
        .skills
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| GatewayError::not_found("Skill", &name))?;

    Ok(Json(json!({
        "name": skill.name,
        "description": skill.description,
        "content": skill.content,
        "path": skill.path.to_string_lossy(),
    })))
}

#[derive(Debug, Deserialize)]
struct CreateSkillBody {
    name: String,
    content: String,
    #[serde(default)]
    description: Option<String>,
}

async fn create_skill(
    State(_state): State<AppState>,
    Json(body): Json<CreateSkillBody>,
) -> Result<Json<Value>, GatewayError> {
    let name = body
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    if name.is_empty() {
        return Err(GatewayError::Unprocessable("skill name required".into()));
    }

    let loader = loader();
    let skill_dir = loader.root().join("skills").join(&name);
    std::fs::create_dir_all(&skill_dir).map_err(|e| GatewayError::Internal(e.to_string()))?;
    let path = skill_dir.join("SKILL.md");
    let mut content = body.content;
    if !content.contains("agentskills.io") {
        content = format!("<!-- agentskills.io -->\n{content}");
    }
    std::fs::write(&path, &content).map_err(|e| GatewayError::Internal(e.to_string()))?;

    Ok(Json(json!({
        "name": name,
        "description": body.description,
        "path": path.to_string_lossy(),
        "created": true,
    })))
}
