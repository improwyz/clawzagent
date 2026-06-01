//! Generic inbound webhooks — `POST /webhooks/{channel_type}/{channel_id}`.

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use serde_json::json;

use crate::routes::channel_inbound::{load_channel, process_webhook_body, send_reply};
use crate::{AppState, GatewayError};

pub fn routes() -> Router<AppState> {
    Router::new().route("/{channel_type}/{channel_id}", post(generic_webhook))
}

async fn generic_webhook(
    State(state): State<AppState>,
    Path((channel_type, channel_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, GatewayError> {
    // Twilio and Google Voice use dedicated handlers with provider-specific auth.
    if channel_type == "twilio" || channel_type == "google-voice" || channel_type == "google_voice"
    {
        return Err(GatewayError::not_found(
            "Webhook route",
            &format!("{channel_type}/{channel_id}"),
        ));
    }

    let record = load_channel(&state, &channel_id).await?;
    if record.channel_type != channel_type && record.channel_type.replace('_', "-") != channel_type
    {
        return Err(GatewayError::Unprocessable(format!(
            "channel {} type mismatch (expected {})",
            channel_id, record.channel_type
        )));
    }

    let replies = process_webhook_body(&state, &record, &channel_type, &body, &headers).await?;
    let count = replies.len();
    for (to, content) in replies {
        send_reply(&state, &record, &to, &content).await?;
    }

    Ok((
        StatusCode::OK,
        axum::Json(json!({ "ok": true, "replies": count })),
    ))
}
