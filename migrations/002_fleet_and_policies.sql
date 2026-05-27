-- Fleet nodes and governance policies (idempotent; mirrored in clawz-core::db::MIGRATIONS_SQL)

CREATE TABLE IF NOT EXISTS fleet_nodes (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    node_type   TEXT NOT NULL DEFAULT 'worker',
    host        TEXT NOT NULL,
    port        INT NOT NULL DEFAULT 8080,
    status      TEXT NOT NULL DEFAULT 'online',
    agent_ids   JSONB NOT NULL DEFAULT '[]',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_fleet_nodes_status ON fleet_nodes(status);

CREATE TABLE IF NOT EXISTS governance_policies (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    rules       JSONB NOT NULL DEFAULT '[]',
    enforcement TEXT NOT NULL DEFAULT 'audit',
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_governance_policies_enabled ON governance_policies(enabled);
