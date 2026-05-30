-- Scheduled agent jobs (micro/elastic). Standalone uses ~/.clawz/cron/jobs.json.

CREATE TABLE IF NOT EXISTS clawz_cron_jobs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    cron_expr TEXT NOT NULL,
    prompt TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    delivery JSONB,
    disabled_toolsets TEXT[] NOT NULL DEFAULT '{}',
    last_run_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_clawz_cron_jobs_agent ON clawz_cron_jobs (agent_id);
