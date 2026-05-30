//! Background poller for channels configured with `poll_interval_secs`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clawz_services::dto::ChannelPollRequest;
use tokio::sync::RwLock;

use crate::routes::channel_inbound::{dispatch_parsed_messages, send_reply};
use crate::AppState;

struct PollState {
    last_tick: RwLock<HashMap<String, Instant>>,
}

impl PollState {
    fn new() -> Self {
        Self {
            last_tick: RwLock::new(HashMap::new()),
        }
    }

    async fn due(&self, channel_id: &str, interval: Duration) -> bool {
        let now = Instant::now();
        let mut map = self.last_tick.write().await;
        match map.get(channel_id) {
            Some(last) if now.duration_since(*last) < interval => false,
            _ => {
                map.insert(channel_id.to_string(), now);
                true
            }
        }
    }
}

/// Start the channel supervisor loop unless disabled via `CLAWZ_CHANNEL_SUPERVISOR=0`.
pub fn spawn(state: Arc<AppState>) {
    if std::env::var("CLAWZ_CHANNEL_SUPERVISOR").ok().as_deref() == Some("0") {
        tracing::info!("channel supervisor disabled (CLAWZ_CHANNEL_SUPERVISOR=0)");
        return;
    }

    let poll_state = Arc::new(PollState::new());
    tokio::spawn(async move {
        let tick_secs = std::env::var("CLAWZ_CHANNEL_SUPERVISOR_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);
        let mut interval = tokio::time::interval(Duration::from_secs(tick_secs));
        tracing::info!("channel supervisor started (tick every {tick_secs}s)");
        loop {
            interval.tick().await;
            if let Err(e) = supervisor_tick(&state, &poll_state).await {
                tracing::warn!("channel supervisor tick: {e}");
            }
        }
    });
}

async fn supervisor_tick(state: &AppState, poll_state: &PollState) -> anyhow::Result<()> {
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("platform not configured"))?;

    let channels = state.channels.read().await.clone();
    for record in channels {
        if !record.enabled {
            continue;
        }
        let Some(secs) = record
            .config
            .get("poll_interval_secs")
            .and_then(|v| v.as_u64())
            .filter(|&n| n > 0)
        else {
            continue;
        };
        if !poll_state
            .due(&record.id, Duration::from_secs(secs))
            .await
        {
            continue;
        }

        let poll = platform
            .execution
            .poll_channel(ChannelPollRequest {
                channel_type: record.channel_type.clone(),
                config: record.config.clone(),
                agent_id: crate::routes::channel_inbound::agent_id_from_config(&record.config),
            })
            .await?;

        if poll.messages.is_empty() {
            continue;
        }

        let replies = dispatch_parsed_messages(state, &record, &record.channel_type, poll.messages)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let reply_count = replies.len();
        for (to, content) in replies {
            send_reply(state, &record, &to, &content)
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        tracing::debug!(
            channel_id = %record.id,
            replies = reply_count,
            "channel supervisor poll completed"
        );
    }
    Ok(())
}
