-- Gateway user accounts, API keys, and communication channels (idempotent)

CREATE TABLE IF NOT EXISTS gateway_users (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL DEFAULT 'default',
    email         TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'user',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_gateway_users_tenant_email
    ON gateway_users(tenant_id, email);

CREATE TABLE IF NOT EXISTS gateway_api_keys (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id  TEXT NOT NULL DEFAULT 'default',
    user_id    UUID NOT NULL REFERENCES gateway_users(id) ON DELETE CASCADE,
    key_hash   TEXT NOT NULL,
    label      TEXT NOT NULL DEFAULT 'default',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_gateway_api_keys_user ON gateway_api_keys(user_id);

CREATE TABLE IF NOT EXISTS gateway_channels (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL DEFAULT 'default',
    name         TEXT NOT NULL,
    channel_type TEXT NOT NULL,
    config       JSONB NOT NULL DEFAULT '{}',
    enabled      BOOLEAN NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_gateway_channels_tenant ON gateway_channels(tenant_id);
