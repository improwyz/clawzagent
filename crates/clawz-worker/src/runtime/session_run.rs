//! Run session execution — single entry point for agent turns (REST, channels, CLI).
//!
//! Loads durable transcripts, handles `/new` `/reset` `/compact` `/usage`, then
//! delegates to [`AgentRuntime::run_multi_turn_in_conversation`].

use clawz_core::types::message::{Message, Role};
use clawz_services::dto::{RunTurnRequest, RunTurnResponse};
use uuid::Uuid;

use crate::runtime::session_commands::{self, SessionCommand};
use crate::service::WorkerService;
use clawz_core::error::Result;

/// Execute one user message through session commands or the full multi-turn tool loop.
pub async fn execute_agent_turn(
    service: &WorkerService,
    agent_id: &str,
    req: RunTurnRequest,
) -> Result<RunTurnResponse> {
    let conversation_id = req
        .conversation_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    match session_commands::parse_command(&req.message) {
        SessionCommand::New => {
            let new_id = Uuid::new_v4().to_string();
            return Ok(command_response(
                agent_id,
                new_id,
                "Started a new session. Use the returned conversation_id on your next message.",
            ));
        }
        SessionCommand::Reset => {
            service.session_store().reset(&conversation_id).await?;
            return Ok(command_response(
                agent_id,
                conversation_id,
                "Session transcript cleared.",
            ));
        }
        SessionCommand::Compact => {
            let removed = service
                .session_store()
                .compact(&conversation_id, session_commands::DEFAULT_COMPACT_KEEP)
                .await?;
            let usage = service.session_store().usage(&conversation_id).await?;
            return Ok(command_response(
                agent_id,
                conversation_id,
                format!(
                    "Compacted session: removed {removed} message(s); {} message(s) remain (~{} tokens).",
                    usage.message_count, usage.estimated_tokens
                ),
            ));
        }
        SessionCommand::Usage => {
            let usage = service.session_store().usage(&conversation_id).await?;
            return Ok(command_response(
                agent_id,
                conversation_id,
                format!(
                    "Session usage: {} message(s), ~{} estimated tokens.",
                    usage.message_count, usage.estimated_tokens
                ),
            ));
        }
        SessionCommand::Chat(user_text) => {
            if user_text.is_empty() {
                return Ok(command_response(
                    agent_id,
                    conversation_id,
                    "Send a message or use /new, /reset, /compact, /usage.",
                ));
            }

            let runtime = service
                .runtime_for(
                    agent_id,
                    req.model.as_deref(),
                    req.system_prompt.as_deref(),
                    req.cron_mode || req.background_mode,
                    &req.disabled_tools,
                )
                .await?;

            let store = service.session_store();
            let mut transcript = store.load_transcript(&conversation_id).await?;
            transcript.push(Message::user(user_text));

            let run_id = crate::runtime::turn_events::TurnEventBus::new_run_id();

            let messages = runtime
                .run_multi_turn_in_conversation(transcript, &conversation_id)
                .await?;

            store.save_transcript(&conversation_id, &messages).await?;

            let restrict = req.cron_mode || req.background_mode;
            if !restrict {
                if let Some(proposal) = crate::workspace::SkillCurator::maybe_propose(&messages) {
                    let name = format!("learned-{}", chrono::Utc::now().timestamp());
                    let loader = crate::workspace::WorkspaceLoader::default_home();
                    let skill_dir = loader.root().join("skills").join(&name);
                    if std::fs::create_dir_all(&skill_dir).is_ok() {
                        let _ = std::fs::write(skill_dir.join("SKILL.md"), proposal);
                    }
                }

                if let Err(e) = service
                    .learning()
                    .transcript_search()
                    .index_session(&conversation_id, &messages)
                    .await
                {
                    log::warn!("[session_run] transcript FTS index failed: {e}");
                }

                if let Some(tenant_id) = req.sender_user_id.as_deref() {
                    if let Err(e) = service
                        .learning()
                        .user_profiles()
                        .touch_session(tenant_id)
                        .await
                    {
                        log::warn!("[session_run] user profile touch failed: {e}");
                    }
                }

                if let Some(assistant_text) = messages
                    .iter()
                    .rev()
                    .find(|m| m.role == Role::Assistant)
                    .and_then(|m| m.content.as_text())
                {
                    if let Err(e) = service
                        .learning()
                        .post_turn_nudge()
                        .nudge(runtime.memory(), agent_id, &conversation_id, assistant_text)
                        .await
                    {
                        log::warn!("[session_run] post-turn memory nudge failed: {e}");
                    }
                }
            }

            service
                .turn_event_bus()
                .emit(crate::runtime::turn_events::TurnEvent::TurnComplete {
                    run_id: run_id.clone(),
                    conversation_id: conversation_id.clone(),
                    turn_count: messages.len() as u32,
                });

            let content = messages
                .iter()
                .rev()
                .find(|m| m.role == Role::Assistant)
                .and_then(|m| m.content.as_text().map(str::to_string))
                .unwrap_or_default();

            Ok(RunTurnResponse {
                agent_id: agent_id.to_string(),
                conversation_id,
                content,
                role: "assistant".to_string(),
                room_id: None,
                message_id: None,
                sender_id: req.sender_user_id,
                delegation_events: None,
                run_id: Some(run_id),
            })
        }
    }
}

fn command_response(
    agent_id: &str,
    conversation_id: String,
    content: impl Into<String>,
) -> RunTurnResponse {
    RunTurnResponse {
        agent_id: agent_id.to_string(),
        conversation_id,
        content: content.into(),
        role: "assistant".to_string(),
        room_id: None,
        message_id: None,
        sender_id: None,
        delegation_events: None,
        run_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_services::dto::RunTurnRequest;

    #[tokio::test]
    async fn test_execute_agent_turn_tool_loop_stub() {
        unsafe {
            std::env::set_var("CLAWZ_STUB_PROVIDER", "1");
        }

        let service = WorkerService::new().await.expect("worker service");
        let agent_id = Uuid::new_v4().to_string();

        let resp = execute_agent_turn(
            &service,
            &agent_id,
            RunTurnRequest {
                message: "CLAWZ_TEST_TOOL_LOOP".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("turn");

        assert!(
            resp.content.contains("tool_loop_ok"),
            "expected tool_loop_ok in content, got: {}",
            resp.content
        );
        assert!(
            resp.content.contains('4'),
            "expected calculator result 4 in content, got: {}",
            resp.content
        );
    }

    #[tokio::test]
    async fn test_session_commands_persist_transcript() {
        let dir = std::env::temp_dir().join(format!("clawz-run-{}", Uuid::new_v4()));
        unsafe {
            std::env::set_var("CLAWZ_HOME", dir.to_string_lossy().as_ref());
            std::env::set_var("CLAWZ_STUB_PROVIDER", "1");
        }

        let service = WorkerService::new().await.expect("worker service");
        let agent_id = Uuid::new_v4().to_string();
        let session_id = Uuid::new_v4().to_string();

        execute_agent_turn(
            &service,
            &agent_id,
            RunTurnRequest {
                message: "hello".to_string(),
                conversation_id: Some(session_id.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("chat turn");

        let usage = service
            .session_store()
            .usage(&session_id)
            .await
            .expect("usage");
        assert!(usage.message_count >= 1);

        let reset = execute_agent_turn(
            &service,
            &agent_id,
            RunTurnRequest {
                message: "/reset".to_string(),
                conversation_id: Some(session_id.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("reset");
        assert!(reset.content.contains("cleared"));

        let usage = service
            .session_store()
            .usage(&session_id)
            .await
            .expect("usage after reset");
        assert_eq!(usage.message_count, 0);
    }
}
