//! Public telephony webhooks — Twilio and Google Voice → agent runs.
//!
//! Configure a channel with `agent_id` in `config`, then point the provider at:
//! - `POST /webhooks/twilio/sms/{channel_id}`
//! - `POST /webhooks/twilio/voice/{channel_id}`
//! - `POST /webhooks/google-voice/{channel_id}`

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::routes::channel_inbound::{load_channel, process_webhook_body, send_reply};
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
    verify_twilio(
        &record,
        &headers,
        &body,
        &format_twilio_url(&state, &channel_id, "sms"),
    )?;

    let replies = process_webhook_body(&state, &record, "twilio", &body, &headers).await?;

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
        process_webhook_body(&state, &record, "twilio", &body, &headers).await?
    } else {
        vec![(
            form.get("From").cloned().unwrap_or_default(),
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

    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/xml")], xml).into_response())
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
            secret, &body, sig,
        )
        .map_err(|e| GatewayError::Unauthorized(e.to_string()))?;
    }

    let replies = process_webhook_body(&state, &record, "google_voice", &body, &headers).await?;
    let count = replies.len();
    for reply in replies {
        send_reply(&state, &record, &reply.0, &reply.1).await?;
    }

    Ok(axum::Json(json!({ "ok": true, "replies": count })))
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
        return Err(GatewayError::Unauthorized(
            "invalid Twilio signature".into(),
        ));
    }
    Ok(())
}

fn parse_form(body: &[u8]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for pair in String::from_utf8_lossy(body).split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(percent_decode(k), percent_decode(v));
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
            if let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
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
    let base = std::env::var("CLAWZ_PUBLIC_URL").unwrap_or_else(|_| "http://localhost:3000".into());
    let base = base.trim_end_matches('/');
    format!("{base}/webhooks/twilio/{kind}/{channel_id}")
}
