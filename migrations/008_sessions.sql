-- Durable session transcripts for micro/elastic (standalone uses ~/.clawz/sessions/).

CREATE TABLE IF NOT EXISTS clawz_sessions (
    session_id TEXT PRIMARY KEY,
    messages JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_clawz_sessions_updated ON clawz_sessions (updated_at DESC);
