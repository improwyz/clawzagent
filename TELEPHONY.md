# Agent telephony — Twilio & Google Voice

ClawZ binds phone numbers directly to agents. Inbound SMS/voice webhooks run an agent turn and send replies on the same channel.

## Prerequisites

- ClawZ installed and running — see **[INSTALL.md](INSTALL.md)** (gateway + worker + Postgres)
- Gateway and worker running with execution client configured
- **`CLAWZ_PUBLIC_URL`** — public HTTPS base (e.g. `https://api.example.com`) used in webhook URLs
- API key for authenticated bind requests

## Bind a phone line to an agent

```http
POST /api/v1/agents/{agent_id}/phone
Authorization: Bearer <api_key>
Content-Type: application/json
```

### Twilio

```json
{
  "provider": "twilio",
  "phone_number": "+15551234567",
  "account_sid": "ACxxxxxxxx",
  "auth_token": "your_auth_token",
  "default_to": "+15559876543"
}
```

Response includes webhook URLs to configure in the Twilio console:

- **SMS:** `{CLAWZ_PUBLIC_URL}/webhooks/twilio/sms/{channel_id}`
- **Voice:** `{CLAWZ_PUBLIC_URL}/webhooks/twilio/voice/{channel_id}`

### Google Voice

```json
{
  "provider": "google_voice",
  "phone_number": "+15551234567",
  "bridge_secret": "long-random-secret"
}
```

Response:

- **Inbound:** `{CLAWZ_PUBLIC_URL}/webhooks/google-voice/{channel_id}`

The Google Voice bridge sends signed JSON payloads to the inbound webhook with `X-Clawz-Signature: sha256=<hmac>` using `bridge_secret`.

## Channel config fields

Stored on the created channel's `config` object:

| Field | Providers | Description |
|-------|-----------|-------------|
| `agent_id` | both | Agent that receives inbound messages |
| `phone_number` | both | E.164 line for this agent |
| `account_sid` | Twilio | Account SID |
| `auth_token` | Twilio | Auth token (webhook signature validation) |
| `bridge_secret` | Google Voice | HMAC secret for `X-Clawz-Signature` |
| `default_to` | optional | Default outbound recipient for tests |
| `voice_twiml_url` | Twilio voice | TwiML URL for outbound calls |

## Manual channel setup

Alternatively create a channel via `POST /api/v1/channels` with `channel_type` `twilio` or `google_voice` and the same `config` fields, then point the provider at the webhook URLs above.

## Outbound from agents

Worker channel `send` uses metadata:

- **SMS:** `metadata.to` = recipient E.164
- **Voice:** `metadata.action` = `call`, `metadata.to`, optional `voice_twiml_url`

Google Voice outbound SMS requires Twilio credentials on the same channel config (no public GV send API).

## Security

- Twilio webhooks: `X-Twilio-Signature` validated with `auth_token`
- Google Voice bridge: `X-Clawz-Signature: sha256=<hmac>` with `bridge_secret`
- Webhook routes are public (no API key); signatures are required
