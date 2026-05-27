//! Public telephony webhooks — Twilio and Google Voice → agent runs.
//!
//! Configure a channel with `agent_id` in `config`, then point the provider at:
//! - `POST /webhooks/twilio/sms/{channel_id}`
//! - `POST /webhooks/twilio/voice/{channel_id}`
//! - `POST /webhooks/google-voice/{channel_id}`

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use base64::Engine;
use clawz_services::dto::{
    ChannelSendRequest, ChannelWebhookRequest, RunTurnRequest,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::telephony::{twiml_empty, twiml_say_and_gather, verify_twilio_signature};
use crate::{AppState, ChannelRecord, GatewayError};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/twilio/sms/{channel_id}", post(twilio_sms))
        .route("/twilio/voice/{channel_id}", post(twilio_voice))
        .route("/google-voice/{channel_id}", post(google_voice))
}

async fn twilio_sms(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, GatewayError> {
    let record = load_channel(&state, &channel_id).await?;
    verify_twilio(&record, &headers, &body, &format_twilio_url(&state, &channel_id, "sms"))?;

    let replies = dispatch_inbound(&state, &record, &body, &headers, "twilio").await?;

    for reply in replies {
        send_reply(&state, &record, &reply.0, &reply.1).await?;
    }

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/xml")],
        twiml_empty(),
    )
        .into_response())
}

async fn twilio_voice(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, GatewayError> {
    let record = load_channel(&state, &channel_id).await?;
    verify_twilio(
        &record,
        &headers,
        &body,
        &format_twilio_url(&state, &channel_id, "voice"),
    )?;

    let form = parse_form(&body);
    let speech = form.get("SpeechResult").cloned();
    let is_gather = speech.is_some();

    let replies = if is_gather {
        dispatch_inbound(&state, &record, &body, &headers, "twilio").await?
    } else {
        vec![(
            form.get("From")
                .cloned()
                .unwrap_or_default(),
            "Hello. How can I help you today?".into(),
        )]
    };

    let say = replies
        .last()
        .map(|(_, text)| text.clone())
        .unwrap_or_else(|| "Goodbye.".into());

    let gather_url = format_twilio_url(&state, &channel_id, "voice");
    let xml = twiml_say_and_gather(&say, &gather_url);

    if is_gather {
        if let Some((to, content)) = replies.last() {
            if !to.is_empty() && !content.is_empty() {
                let _ = send_reply(&state, &record, to, content).await;
            }
        }
    }

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/xml")],
        xml,
    )
        .into_response())
}

async fn google_voice(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<axum::Json<Value>, GatewayError> {
    let record = load_channel(&state, &channel_id).await?;
    if record.channel_type != "google_voice" {
        return Err(GatewayError::Unprocessable(format!(
            "channel {} is not google_voice",
            record.id
        )));
    }

    if let Some(secret) = record.config.get("bridge_secret").and_then(|v| v.as_str()) {
        let sig = headers
            .get("x-clawz-signature")
            .or_else(|| headers.get("X-Clawz-Signature"))
            .and_then(|v| v.to_str().ok());
        clawz_worker::channels::native::google_voice::verify_google_voice_signature(
            secret,
            &body,
            sig,
        )
        .map_err(|e| GatewayError::Unauthorized(e.to_string()))?;
    }

    let replies = dispatch_inbound(&state, &record, &body, &headers, "google_voice").await?;
    let count = replies.len();
    for reply in replies {
        send_reply(&state, &record, &reply.0, &reply.1).await?;
    }

    Ok(axum::Json(json!({ "ok": true, "replies": count })))
}

async fn load_channel(state: &AppState, channel_id: &str) -> Result<ChannelRecord, GatewayError> {
    let channels = state.channels.read().await;
    let record = channels
        .iter()
        .find(|c| c.id == channel_id)
        .cloned()
        .ok_or_else(|| GatewayError::not_found("Channel", channel_id))?;
    if !record.enabled {
        return Err(GatewayError::Unprocessable("channel is disabled".into()));
    }
    Ok(record)
}

fn agent_id_from_config(config: &Value) -> Option<String> {
    config
        .get("agent_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

async fn dispatch_inbound(
    state: &AppState,
    record: &ChannelRecord,
    body: &[u8],
    headers: &HeaderMap,
    platform: &str,
) -> Result<Vec<(String, String)>, GatewayError> {
    let platform_exec = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let mut header_map = std::collections::HashMap::new();
    for (k, v) in headers.iter() {
        if let (Ok(name), Ok(val)) = (k.as_str().parse::<String>(), v.to_str()) {
            header_map.insert(name, val.to_string());
        }
    }

    let parsed = platform_exec
        .execution
        .process_channel_webhook(ChannelWebhookRequest {
            channel_type: platform.to_string(),
            config: record.config.clone(),
            agent_id: agent_id_from_config(&record.config),
            body_base64: base64::engine::general_purpose::STANDARD.encode(body),
            content_type: headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            headers: header_map,
        })
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    let agent_id = agent_id_from_config(&record.config).ok_or_else(|| {
        GatewayError::Unprocessable("channel config missing agent_id".into())
    })?;

    let mut replies = Vec::new();
    for msg in parsed.messages {
        if msg.content.trim().is_empty() {
            continue;
        }
        let turn = platform_exec
            .execution
            .run_turn(
                &agent_id,
                RunTurnRequest {
                    message: msg.content.clone(),
                    model: None,
                    system_prompt: None,
                    conversation_id: None,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        replies.push((msg.from, turn.content));
    }
    Ok(replies)
}

async fn send_reply(
    state: &AppState,
    record: &ChannelRecord,
    to: &str,
    content: &str,
) -> Result<(), GatewayError> {
    let platform = state.platform.as_ref().ok_or_else(|| {
        GatewayError::Internal("worker platform not configured".into())
    })?;

    let metadata = json!({ "to": to });
    platform
        .execution
        .send_channel_message(ChannelSendRequest {
            channel_type: record.channel_type.clone(),
            config: record.config.clone(),
            content: content.to_string(),
            metadata,
            agent_id: agent_id_from_config(&record.config),
        })
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;
    Ok(())
}

fn verify_twilio(
    record: &ChannelRecord,
    headers: &HeaderMap,
    body: &[u8],
    url: &str,
) -> Result<(), GatewayError> {
    let token = record
        .config
        .get("auth_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GatewayError::Unprocessable("Twilio auth_token missing".into()))?;

    let signature = headers
        .get("X-Twilio-Signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| GatewayError::Unauthorized("missing X-Twilio-Signature".into()))?;

    let params = parse_form(body);
    if !verify_twilio_signature(token, url, &params, signature) {
        return Err(GatewayError::Unauthorized("invalid Twilio signature".into()));
    }
    Ok(())
}

fn parse_form(body: &[u8]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for pair in String::from_utf8_lossy(body).split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(
                percent_decode(k),
                percent_decode(v),
            );
        }
    }
    map
}

fn percent_decode(s: &str) -> String {
    let replaced = s.replace('+', " ");
    let mut out = String::new();
    let bytes = replaced.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                out.push(byte as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn format_twilio_url(_state: &AppState, channel_id: &str, kind: &str) -> String {
    let base =
        std::env::var("CLAWZ_PUBLIC_URL").unwrap_or_else(|_| "http://localhost:3000".into());
    let base = base.trim_end_matches('/');
    format!("{base}/webhooks/twilio/{kind}/{channel_id}")
}
