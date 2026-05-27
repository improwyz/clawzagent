//! Postgres persistence helpers for gateway CRUD.

use chrono::Utc;
use clawz_core::db::{
    AgentRepo, ConversationRepo, DbAgent, DbConversation, DbDeployment, DbFleetNode,
    DbGatewayApiKey, DbGatewayAudit, DbGatewayChannel, DbGatewayProvider, DbGatewayTool,
    DbGatewayUser, DbGovernancePolicy, DbMessage, DbOrchestrationRun, DbRoom, DbRoomParticipant,
    DeploymentRepo, FleetNodeRepo, GatewayApiKeyRepo, GatewayAuditRepo, GatewayChannelRepo,
    GatewayProviderRepo, GatewayToolRepo, GatewayUserRepo, GovernancePolicyRepo, MessageRepo,
    OrchestrationRunRepo, RoomParticipantRepo, RoomRepo,
};
use clawz_core::types::agent::AgentConfig;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::deploy::provider::DeploymentInfo;
use crate::{
    AgentRecord, ApiKeyRecord, AuditEntry, ChannelRecord, ConversationRecord, FleetNodeRecord,
    MessageRecord, PolicyRecord, ProviderRecord, RoomMessageRecord, RoomParticipantRecord,
    RoomRecord, RoomSideThreadRecord, ToolRecord, UserRecord,
};

/// Persist a gateway [`AgentRecord`] to Postgres when a pool is configured.
pub async fn persist_agent(pool: &PgPool, record: &AgentRecord) -> clawz_core::Result<()> {
    let id = Uuid::parse_str(&record.id).unwrap_or_else(|_| Uuid::new_v4());
    let mut config = AgentConfig::new(&record.name, &record.model);
    config.id = id;
    if let Some(ref prompt) = record.system_prompt {
        config = config.with_system_prompt(prompt);
    }
    let config_json = serde_json::to_value(&config)
        .map_err(|e| clawz_core::error::ClawzError::Config(e.to_string()))?;

    let db_agent = DbAgent {
        id,
        name: record.name.clone(),
        config: config_json,
        status: record.status.to_string(),
        created_at: record.created_at,
        updated_at: record.updated_at,
    };

    AgentRepo::insert(pool, &db_agent).await
}

/// Load agents from Postgres into gateway records (startup hydration).
pub async fn load_agents(pool: &PgPool) -> clawz_core::Result<Vec<AgentRecord>> {
    let rows = AgentRepo::list(pool, 10_000, 0).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let model = row
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let system_prompt = row
            .config
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let description = row
            .config
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let status = match row.status.as_str() {
            "running" => crate::AgentStatus::Running,
            "stopped" => crate::AgentStatus::Stopped,
            "error" => crate::AgentStatus::Error,
            _ => crate::AgentStatus::Idle,
        };
        out.push(AgentRecord {
            id: row.id.to_string(),
            name: row.name,
            model,
            description,
            system_prompt,
            status,
            created_at: row.created_at,
            updated_at: row.updated_at,
        });
    }
    Ok(out)
}

/// Insert message rows when DATABASE_URL is set (best-effort).
pub async fn persist_messages(
    pool: &PgPool,
    conversation_id: &str,
    user_message: &str,
    assistant_message: &str,
) {
    let conv_id = Uuid::parse_str(conversation_id).unwrap_or_else(|_| Uuid::new_v4());
    let now = Utc::now();
    let _ = MessageRepo::insert(
        pool,
        &DbMessage {
            id: Uuid::new_v4(),
            conversation_id: conv_id,
            room_id: None,
            seq: 0,
            role: "user".into(),
            content: json!({"text": user_message}),
            sender_type: Some("user".into()),
            sender_id: None,
            message_kind: "user_text".into(),
            visibility: "room".into(),
            parent_message_id: None,
            target_agent_ids: json!([]),
            client_message_id: None,
            created_at: now,
        },
    )
    .await;
    let _ = MessageRepo::insert(
        pool,
        &DbMessage {
            id: Uuid::new_v4(),
            conversation_id: conv_id,
            room_id: None,
            seq: 0,
            role: "assistant".into(),
            content: json!({"text": assistant_message}),
            sender_type: Some("agent".into()),
            sender_id: None,
            message_kind: "agent_text".into(),
            visibility: "room".into(),
            parent_message_id: None,
            target_agent_ids: json!([]),
            client_message_id: None,
            created_at: now,
        },
    )
    .await;
}

/// Persist a conversation header row (messages stored separately).
pub async fn persist_conversation(
    pool: &PgPool,
    record: &ConversationRecord,
) -> clawz_core::Result<()> {
    let conv_id = Uuid::parse_str(&record.id).unwrap_or_else(|_| Uuid::new_v4());
    let agent_id = Uuid::parse_str(&record.agent_id).unwrap_or_else(|_| Uuid::new_v4());
    let metadata = json!({
        "title": record.title,
        "archived": record.archived,
    });
    let db_conv = DbConversation {
        id: conv_id,
        agent_id,
        channel_id: None,
        started_at: record.created_at,
        last_message_at: record.updated_at,
        metadata,
    };
    ConversationRepo::upsert(pool, &db_conv).await
}

/// Persist a single message and bump conversation `last_message_at`.
pub async fn persist_message(pool: &PgPool, msg: &MessageRecord) {
    let conv_id = Uuid::parse_str(&msg.conversation_id).unwrap_or_else(|_| Uuid::new_v4());
    let _ = MessageRepo::insert(
        pool,
        &DbMessage {
            id: Uuid::parse_str(&msg.id).unwrap_or_else(|_| Uuid::new_v4()),
            conversation_id: conv_id,
            room_id: None,
            seq: 0,
            role: msg.role.clone(),
            content: json!({"text": msg.content}),
            sender_type: Some(msg.role.clone()),
            sender_id: None,
            message_kind: if msg.role == "assistant" {
                "agent_text".into()
            } else {
                "user_text".into()
            },
            visibility: "room".into(),
            parent_message_id: None,
            target_agent_ids: json!([]),
            client_message_id: None,
            created_at: msg.created_at,
        },
    )
    .await;
    let _ = ConversationRepo::touch(pool, conv_id).await;
}

/// Load conversations and their messages for gateway startup hydration.
pub async fn load_conversations(pool: &PgPool) -> clawz_core::Result<Vec<ConversationRecord>> {
    let rows = ConversationRepo::list_recent(pool, 5_000).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let title = row
            .metadata
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let archived = row
            .metadata
            .get("archived")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let messages = MessageRepo::list_for_conversation(pool, row.id, 10_000)
            .await?
            .into_iter()
            .map(|m| MessageRecord {
                id: m.id.to_string(),
                conversation_id: m.conversation_id.to_string(),
                role: m.role,
                content: m
                    .content
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                created_at: m.created_at,
            })
            .collect();
        out.push(ConversationRecord {
            id: row.id.to_string(),
            agent_id: row.agent_id.to_string(),
            title,
            archived,
            messages,
            created_at: row.started_at,
            updated_at: row.last_message_at,
        });
    }
    Ok(out)
}

/// Persist a gateway audit entry (all resource types).
pub async fn persist_audit(pool: &PgPool, entry: &AuditEntry) {
    let db_entry = DbGatewayAudit {
        id: Uuid::parse_str(&entry.id).unwrap_or_else(|_| Uuid::new_v4()),
        actor: entry.actor.clone(),
        action: entry.action.clone(),
        resource_type: entry.resource_type.clone(),
        resource_id: entry.resource_id.clone(),
        details: entry.details.clone(),
        created_at: entry.created_at,
    };
    let _ = GatewayAuditRepo::insert(pool, &db_entry).await;
}

/// Paginated audit query for `GET /governance/audit` when Postgres is configured.
pub async fn query_audit_filtered(
    pool: &PgPool,
    resource_type: Option<&str>,
    actor: Option<&str>,
    limit: i64,
    offset: i64,
) -> clawz_core::Result<(Vec<AuditEntry>, i64)> {
    let (rows, total) =
        GatewayAuditRepo::list_filtered(pool, resource_type, actor, limit, offset).await?;
    let entries = rows
        .into_iter()
        .map(|row| AuditEntry {
            id: row.id.to_string(),
            actor: row.actor,
            action: row.action,
            resource_type: row.resource_type,
            resource_id: row.resource_id,
            details: row.details,
            created_at: row.created_at,
        })
        .collect();
    Ok((entries, total))
}

pub async fn load_audit_entries(pool: &PgPool) -> clawz_core::Result<Vec<AuditEntry>> {
    let rows = GatewayAuditRepo::list_recent(pool, 10_000).await?;
    Ok(rows
        .into_iter()
        .map(|row| AuditEntry {
            id: row.id.to_string(),
            actor: row.actor,
            action: row.action,
            resource_type: row.resource_type,
            resource_id: row.resource_id,
            details: row.details,
            created_at: row.created_at,
        })
        .collect())
}

pub async fn persist_fleet_node(pool: &PgPool, record: &FleetNodeRecord) -> clawz_core::Result<()> {
    let id = Uuid::parse_str(&record.id).unwrap_or_else(|_| Uuid::new_v4());
    let agent_ids = serde_json::to_value(&record.agent_ids)
        .map_err(|e| clawz_core::error::ClawzError::Config(e.to_string()))?;
    FleetNodeRepo::upsert(
        pool,
        &DbFleetNode {
            id,
            name: record.name.clone(),
            node_type: record.node_type.clone(),
            host: record.host.clone(),
            port: i32::from(record.port),
            status: record.status.clone(),
            agent_ids,
            created_at: record.created_at,
            updated_at: record.updated_at,
        },
    )
    .await
}

pub async fn load_fleet_nodes(pool: &PgPool) -> clawz_core::Result<Vec<FleetNodeRecord>> {
    let rows = FleetNodeRepo::list(pool, 5_000).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let agent_ids: Vec<String> =
                serde_json::from_value(row.agent_ids.clone()).unwrap_or_default();
            FleetNodeRecord {
                id: row.id.to_string(),
                name: row.name,
                node_type: row.node_type,
                host: row.host,
                port: u16::try_from(row.port).unwrap_or(8080),
                status: row.status,
                agent_ids,
                created_at: row.created_at,
                updated_at: row.updated_at,
            }
        })
        .collect())
}

pub async fn persist_policy(pool: &PgPool, record: &PolicyRecord) -> clawz_core::Result<()> {
    let id = Uuid::parse_str(&record.id).unwrap_or_else(|_| Uuid::new_v4());
    let rules = serde_json::to_value(&record.rules)
        .map_err(|e| clawz_core::error::ClawzError::Config(e.to_string()))?;
    GovernancePolicyRepo::upsert(
        pool,
        &DbGovernancePolicy {
            id,
            name: record.name.clone(),
            description: record.description.clone(),
            rules,
            enforcement: record.enforcement.clone(),
            enabled: record.enabled,
            created_at: record.created_at,
            updated_at: record.updated_at,
        },
    )
    .await
}

pub async fn delete_policy(pool: &PgPool, id: &str) {
    if let Ok(uuid) = Uuid::parse_str(id) {
        let _ = GovernancePolicyRepo::delete(pool, uuid).await;
    }
}

pub async fn load_policies(pool: &PgPool) -> clawz_core::Result<Vec<PolicyRecord>> {
    let rows = GovernancePolicyRepo::list(pool, 5_000).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let rules: Vec<String> = serde_json::from_value(row.rules).unwrap_or_default();
            PolicyRecord {
                id: row.id.to_string(),
                name: row.name,
                description: row.description,
                rules,
                enforcement: row.enforcement,
                enabled: row.enabled,
                created_at: row.created_at,
                updated_at: row.updated_at,
            }
        })
        .collect())
}

pub async fn persist_provider(pool: &PgPool, record: &ProviderRecord) -> clawz_core::Result<()> {
    let id = Uuid::parse_str(&record.id).unwrap_or_else(|_| Uuid::new_v4());
    let api_key = record
        .api_key
        .as_ref()
        .map(|k| crate::secrets::seal_secret(k));
    GatewayProviderRepo::upsert(
        pool,
        &DbGatewayProvider {
            id,
            name: record.name.clone(),
            provider_type: record.provider_type.clone(),
            api_key,
            base_url: record.base_url.clone(),
            enabled: record.enabled,
            created_at: record.created_at,
            updated_at: record.updated_at,
        },
    )
    .await
}

pub async fn load_providers(pool: &PgPool) -> clawz_core::Result<Vec<ProviderRecord>> {
    let rows = GatewayProviderRepo::list(pool, 5_000).await?;
    Ok(rows
        .into_iter()
        .map(|row| ProviderRecord {
            id: row.id.to_string(),
            name: row.name,
            provider_type: row.provider_type,
            api_key: row
                .api_key
                .as_ref()
                .and_then(|k| crate::secrets::open_secret(k)),
            base_url: row.base_url,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect())
}

pub async fn delete_provider(pool: &PgPool, id: &str) {
    if let Ok(uuid) = Uuid::parse_str(id) {
        let _ = GatewayProviderRepo::delete(pool, uuid).await;
    }
}

pub async fn persist_tool(pool: &PgPool, record: &ToolRecord) -> clawz_core::Result<()> {
    let id = Uuid::parse_str(&record.id).unwrap_or_else(|_| Uuid::new_v4());
    GatewayToolRepo::upsert(
        pool,
        &DbGatewayTool {
            id,
            name: record.name.clone(),
            description: record.description.clone(),
            tool_type: record.tool_type.clone(),
            config: crate::secrets::seal_sensitive_json(&record.config),
            enabled: record.enabled,
            created_at: record.created_at,
            updated_at: record.updated_at,
        },
    )
    .await
}

pub async fn load_tools(pool: &PgPool) -> clawz_core::Result<Vec<ToolRecord>> {
    let rows = GatewayToolRepo::list(pool, 5_000).await?;
    Ok(rows
        .into_iter()
        .map(|row| ToolRecord {
            id: row.id.to_string(),
            name: row.name,
            description: row.description,
            tool_type: row.tool_type,
            config: crate::secrets::open_sensitive_json(&row.config),
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect())
}

/// Persist a cloud deployment row (`deployments` table).
pub async fn persist_cloud_deployment(
    pool: &PgPool,
    info: &DeploymentInfo,
) -> clawz_core::Result<()> {
    let id = crate::deploy::common::deployment_db_uuid(&info.id);
    let now = Utc::now();
    let mut config = json!({
        "gateway_deployment_id": info.id,
        "provider_id": info.provider_id,
    });
    if let Some(ref ext) = info.external_resource {
        if let Some(obj) = config.as_object_mut() {
            obj.insert("external_resource".into(), json!(ext));
        }
    }
    DeploymentRepo::insert(
        pool,
        &DbDeployment {
            id,
            provider: info.provider_id.clone(),
            config,
            status: crate::deploy::common::deployment_status_to_str(info.status).to_string(),
            url: Some(info.url.clone()),
            created_at: now,
            updated_at: now,
        },
    )
    .await
}

/// Remove a cloud deployment row after provider teardown.
pub async fn delete_cloud_deployment(pool: &PgPool, deployment_id: &str) -> clawz_core::Result<()> {
    let uuid = crate::deploy::common::deployment_db_uuid(deployment_id);
    DeploymentRepo::delete(pool, uuid).await
}

/// Load cloud deployments for gateway startup hydration.
pub async fn load_cloud_deployments(pool: &PgPool) -> clawz_core::Result<Vec<DeploymentInfo>> {
    let rows = DeploymentRepo::list_all(pool, 5_000).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let gateway_id = row
                .config
                .get("gateway_deployment_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| row.id.to_string());
            let provider_id = row
                .config
                .get("provider_id")
                .and_then(|v| v.as_str())
                .unwrap_or(row.provider.as_str())
                .to_string();
            DeploymentInfo {
                id: gateway_id,
                provider_id,
                external_resource: row
                    .config
                    .get("external_resource")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                url: row.url.unwrap_or_default(),
                status: crate::deploy::common::deployment_status_from_str(&row.status),
            }
        })
        .collect())
}

pub async fn delete_tool(pool: &PgPool, id: &str) {
    if let Ok(uuid) = Uuid::parse_str(id) {
        let _ = GatewayToolRepo::delete(pool, uuid).await;
    }
}

fn room_gateway_metadata(record: &RoomRecord) -> serde_json::Value {
    json!({
        "created_by": record.created_by,
        "side_threads": record.side_threads,
        "orchestration_config": record.orchestration_config,
    })
}

fn room_record_from_db(
    row: DbRoom,
    participants: Vec<RoomParticipantRecord>,
    messages: Vec<RoomMessageRecord>,
) -> RoomRecord {
    let created_by = row
        .metadata
        .get("created_by")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let side_threads: Vec<RoomSideThreadRecord> = row
        .metadata
        .get("side_threads")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let orchestration_config = row
        .metadata
        .get("orchestration_config")
        .cloned()
        .filter(|v| !v.is_null());
    RoomRecord {
        id: row.id.to_string(),
        tenant_id: row.tenant_id,
        room_type: row.room_type,
        orchestration_mode: Some(row.orchestration_mode),
        orchestration_config,
        participants,
        messages,
        side_threads,
        created_by,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn participant_from_db(row: DbRoomParticipant) -> RoomParticipantRecord {
    RoomParticipantRecord {
        participant_id: row.participant_id,
        participant_type: row.participant_type,
        role: row.role,
        joined_at: row.joined_at,
    }
}

fn room_message_from_db(row: DbMessage) -> RoomMessageRecord {
    let content = row
        .content
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mentions: Vec<String> = row
        .content
        .get("mentions")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .or_else(|| serde_json::from_value(row.target_agent_ids.clone()).ok())
        .unwrap_or_default();
    let thread_id = row
        .content
        .get("thread_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    RoomMessageRecord {
        id: row.id.to_string(),
        room_id: row.room_id.map(|id| id.to_string()).unwrap_or_default(),
        seq: u64::try_from(row.seq).unwrap_or(0),
        sender_type: row.sender_type.unwrap_or_else(|| "user".to_string()),
        sender_id: row.sender_id.unwrap_or_default(),
        client_message_id: row.client_message_id.map(|id| id.to_string()),
        content,
        mentions,
        visibility: row.visibility,
        thread_id,
        created_at: row.created_at,
    }
}

fn room_to_db(record: &RoomRecord) -> DbRoom {
    let room_id = Uuid::parse_str(&record.id).unwrap_or_else(|_| Uuid::new_v4());
    let mut metadata = room_gateway_metadata(record);
    if let Some(obj) = metadata.as_object_mut() {
        obj.insert("gateway".into(), json!({ "version": 1 }));
    }
    DbRoom {
        id: room_id,
        tenant_id: record.tenant_id.clone(),
        conversation_id: if record.room_type == "direct" {
            Some(room_id)
        } else {
            None
        },
        title: None,
        room_type: record.room_type.clone(),
        orchestration_mode: record
            .orchestration_mode
            .clone()
            .unwrap_or_else(|| "single".to_string()),
        swarm_pattern: None,
        parent_room_id: None,
        visibility: "private".to_string(),
        metadata,
        primary_agent_id: record
            .participants
            .iter()
            .find(|p| p.participant_type == "agent" && p.role == "leader")
            .and_then(|p| Uuid::parse_str(&p.participant_id).ok()),
        created_at: record.created_at,
        updated_at: record.updated_at,
        last_message_at: record.updated_at,
    }
}

fn parse_client_message_id(raw: &Option<String>) -> Option<Uuid> {
    raw.as_ref().and_then(|s| Uuid::parse_str(s).ok())
}

/// Persist an orchestration run record (optional — ignores errors at call sites).
pub async fn persist_orchestration_run(
    pool: &PgPool,
    run: &DbOrchestrationRun,
) -> clawz_core::Result<()> {
    OrchestrationRunRepo::insert(pool, run).await
}

/// Persist a multi-participant room and its participants.
pub async fn persist_room(pool: &PgPool, record: &RoomRecord) -> clawz_core::Result<()> {
    let db_room = room_to_db(record);
    RoomRepo::upsert(pool, &db_room).await?;
    for participant in &record.participants {
        let db_participant = DbRoomParticipant {
            id: Uuid::new_v4(),
            room_id: db_room.id,
            participant_type: participant.participant_type.clone(),
            participant_id: participant.participant_id.clone(),
            role: participant.role.clone(),
            permissions: json!({}),
            joined_at: participant.joined_at,
            left_at: None,
        };
        RoomParticipantRepo::insert(pool, &db_participant).await?;
    }
    Ok(())
}

async fn hydrate_rooms_from_rows(
    pool: &PgPool,
    rows: Vec<DbRoom>,
) -> clawz_core::Result<Vec<RoomRecord>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let participants = RoomParticipantRepo::list_for_room(pool, row.id)
            .await?
            .into_iter()
            .map(participant_from_db)
            .collect();
        let messages = load_room_messages(pool, row.id, 0, 10_000).await?;
        out.push(room_record_from_db(row, participants, messages));
    }
    Ok(out)
}

/// Load rooms for a single tenant (tenant-scoped gateway queries).
pub async fn load_rooms(pool: &PgPool, tenant_id: &str) -> clawz_core::Result<Vec<RoomRecord>> {
    let rows = RoomRepo::list_for_tenant(pool, tenant_id, 5_000, 0).await?;
    hydrate_rooms_from_rows(pool, rows).await
}

/// Load all rooms across tenants for startup hydration (preserves `tenant_id`).
pub async fn load_all_rooms(pool: &PgPool) -> clawz_core::Result<Vec<RoomRecord>> {
    let rows = RoomRepo::list_recent(pool, 5_000).await?;
    hydrate_rooms_from_rows(pool, rows).await
}

fn room_message_to_db(msg: &RoomMessageRecord) -> DbMessage {
    let room_id = Uuid::parse_str(&msg.room_id).unwrap_or_else(|_| Uuid::new_v4());
    let sender_type = msg.sender_type.clone();
    let role = if sender_type == "agent" {
        "assistant"
    } else {
        "user"
    };
    let message_kind = if sender_type == "agent" {
        "agent_text"
    } else {
        "user_text"
    };
    DbMessage {
        id: Uuid::parse_str(&msg.id).unwrap_or_else(|_| Uuid::new_v4()),
        conversation_id: room_id,
        room_id: Some(room_id),
        seq: 0,
        role: role.into(),
        content: json!({
            "text": msg.content,
            "mentions": msg.mentions,
            "thread_id": msg.thread_id,
        }),
        sender_type: Some(sender_type),
        sender_id: Some(msg.sender_id.clone()),
        message_kind: message_kind.into(),
        visibility: msg.visibility.clone(),
        parent_message_id: msg
            .thread_id
            .as_ref()
            .and_then(|id| Uuid::parse_str(id).ok()),
        target_agent_ids: json!(msg.mentions),
        client_message_id: parse_client_message_id(&msg.client_message_id),
        created_at: msg.created_at,
    }
}

/// Append a room message via Postgres (server assigns `seq`).
pub async fn append_room_message_db(
    pool: &PgPool,
    msg: &RoomMessageRecord,
) -> clawz_core::Result<RoomMessageRecord> {
    let mut db_msg = room_message_to_db(msg);
    RoomRepo::append_message(pool, &mut db_msg).await?;
    Ok(room_message_from_db(db_msg))
}

/// Persist a single room message (Postgres assigns `seq`).
pub async fn persist_room_message(
    pool: &PgPool,
    msg: &RoomMessageRecord,
) -> clawz_core::Result<RoomMessageRecord> {
    append_room_message_db(pool, msg).await
}

/// Load room messages after a sequence cursor.
pub async fn load_room_messages(
    pool: &PgPool,
    room_id: Uuid,
    after_seq: u64,
    limit: i64,
) -> clawz_core::Result<Vec<RoomMessageRecord>> {
    let after = i64::try_from(after_seq).unwrap_or(0);
    let rows = MessageRepo::list_for_room(pool, room_id, after, limit).await?;
    Ok(rows.into_iter().map(room_message_from_db).collect())
}

fn default_tenant_id() -> String {
    std::env::var("CLAWZ_TENANT_ID").unwrap_or_else(|_| "default".to_string())
}

/// Persist a registered user account.
pub async fn persist_user(
    pool: &PgPool,
    record: &UserRecord,
    tenant_id: &str,
) -> clawz_core::Result<()> {
    let id = Uuid::parse_str(&record.id).unwrap_or_else(|_| Uuid::new_v4());
    GatewayUserRepo::upsert(
        pool,
        &DbGatewayUser {
            id,
            tenant_id: tenant_id.to_string(),
            email: record.email.clone(),
            password_hash: record.password_hash.clone(),
            role: record.role.clone(),
            created_at: record.created_at,
            updated_at: record.updated_at,
        },
    )
    .await
}

/// Load gateway users for startup hydration.
pub async fn load_users(pool: &PgPool) -> clawz_core::Result<Vec<UserRecord>> {
    let rows = GatewayUserRepo::list(pool, 5_000).await?;
    Ok(rows
        .into_iter()
        .map(|row| UserRecord {
            id: row.id.to_string(),
            email: row.email,
            password_hash: row.password_hash,
            role: row.role,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect())
}

/// Look up a user by email within a tenant (login Postgres fallback).
pub async fn find_user_by_email(
    pool: &PgPool,
    tenant_id: &str,
    email: &str,
) -> clawz_core::Result<Option<UserRecord>> {
    let row = GatewayUserRepo::find_by_email(pool, tenant_id, email).await?;
    Ok(row.map(|row| UserRecord {
        id: row.id.to_string(),
        email: row.email,
        password_hash: row.password_hash,
        role: row.role,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

/// Persist a user-issued API key record.
pub async fn persist_api_key(
    pool: &PgPool,
    record: &ApiKeyRecord,
    tenant_id: &str,
) -> clawz_core::Result<()> {
    let id = Uuid::parse_str(&record.id).unwrap_or_else(|_| Uuid::new_v4());
    let user_id = Uuid::parse_str(&record.user_id).unwrap_or_else(|_| Uuid::new_v4());
    GatewayApiKeyRepo::insert(
        pool,
        &DbGatewayApiKey {
            id,
            tenant_id: tenant_id.to_string(),
            user_id,
            key_hash: record.key_hash.clone(),
            label: record.label.clone(),
            created_at: record.created_at,
        },
    )
    .await
}

/// Load user API keys for startup hydration.
pub async fn load_api_keys(pool: &PgPool) -> clawz_core::Result<Vec<ApiKeyRecord>> {
    let rows = GatewayApiKeyRepo::list(pool, 10_000).await?;
    Ok(rows
        .into_iter()
        .map(|row| ApiKeyRecord {
            id: row.id.to_string(),
            user_id: row.user_id.to_string(),
            key_hash: row.key_hash,
            label: row.label,
            created_at: row.created_at,
        })
        .collect())
}

/// Persist a communication channel integration.
pub async fn persist_channel(pool: &PgPool, record: &ChannelRecord) -> clawz_core::Result<()> {
    let id = Uuid::parse_str(&record.id).unwrap_or_else(|_| Uuid::new_v4());
    GatewayChannelRepo::upsert(
        pool,
        &DbGatewayChannel {
            id,
            tenant_id: record.tenant_id.clone(),
            name: record.name.clone(),
            channel_type: record.channel_type.clone(),
            config: crate::secrets::seal_sensitive_json(&record.config),
            enabled: record.enabled,
            created_at: record.created_at,
            updated_at: record.updated_at,
        },
    )
    .await
}

fn channel_record_from_db(row: DbGatewayChannel) -> ChannelRecord {
    ChannelRecord {
        id: row.id.to_string(),
        tenant_id: row.tenant_id,
        name: row.name,
        channel_type: row.channel_type,
        config: crate::secrets::open_sensitive_json(&row.config),
        enabled: row.enabled,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Load communication channels for startup hydration.
pub async fn load_channels(pool: &PgPool) -> clawz_core::Result<Vec<ChannelRecord>> {
    let rows = GatewayChannelRepo::list(pool, 5_000).await?;
    Ok(rows.into_iter().map(channel_record_from_db).collect())
}

/// Load communication channels scoped to a tenant.
pub async fn load_channels_for_tenant(
    pool: &PgPool,
    tenant_id: &str,
) -> clawz_core::Result<Vec<ChannelRecord>> {
    let rows = GatewayChannelRepo::list_for_tenant(pool, tenant_id, 5_000).await?;
    Ok(rows.into_iter().map(channel_record_from_db).collect())
}

/// Delete a channel from Postgres.
pub async fn delete_channel(pool: &PgPool, id: &str) -> clawz_core::Result<()> {
    if let Ok(uuid) = Uuid::parse_str(id) {
        GatewayChannelRepo::delete(pool, uuid).await?;
    }
    Ok(())
}

/// Default tenant for records created without explicit auth context.
pub fn default_tenant() -> String {
    default_tenant_id()
}
