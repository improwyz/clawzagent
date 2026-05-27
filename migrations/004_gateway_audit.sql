-- Gateway audit log (all resource types; idempotent)

CREATE TABLE IF NOT EXISTS gateway_audit (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    actor         TEXT NOT NULL,
    action        TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id   TEXT NOT NULL,
    details       TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_gateway_audit_created ON gateway_audit(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_gateway_audit_resource ON gateway_audit(resource_type, resource_id);
