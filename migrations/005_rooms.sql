-- Multi-participant agent rooms (idempotent; mirrored in clawz-core::db::MIGRATIONS_SQL)

CREATE TABLE IF NOT EXISTS rooms (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id          TEXT NOT NULL DEFAULT 'default',
    conversation_id    UUID UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    title              TEXT,
    room_type          TEXT NOT NULL DEFAULT 'direct',
    orchestration_mode TEXT NOT NULL DEFAULT 'single',
    swarm_pattern      TEXT,
    parent_room_id     UUID REFERENCES rooms(id) ON DELETE SET NULL,
    visibility         TEXT NOT NULL DEFAULT 'public',
    metadata           JSONB NOT NULL DEFAULT '{}',
    primary_agent_id   UUID REFERENCES agents(id) ON DELETE SET NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_message_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_rooms_tenant ON rooms(tenant_id);
CREATE INDEX IF NOT EXISTS idx_rooms_parent ON rooms(parent_room_id);
CREATE INDEX IF NOT EXISTS idx_rooms_last_msg ON rooms(last_message_at DESC);
CREATE INDEX IF NOT EXISTS idx_rooms_type ON rooms(room_type);

CREATE TABLE IF NOT EXISTS room_participants (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    room_id          UUID NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    participant_type TEXT NOT NULL,
    participant_id   TEXT NOT NULL,
    role             TEXT NOT NULL DEFAULT 'member',
    permissions      JSONB NOT NULL DEFAULT '{}',
    joined_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    left_at          TIMESTAMPTZ,
    UNIQUE (room_id, participant_type, participant_id)
);

CREATE INDEX IF NOT EXISTS idx_room_participants_room ON room_participants(room_id);
CREATE INDEX IF NOT EXISTS idx_room_participants_lookup
    ON room_participants(participant_type, participant_id);

ALTER TABLE messages ADD COLUMN IF NOT EXISTS room_id UUID REFERENCES rooms(id) ON DELETE CASCADE;
ALTER TABLE messages ADD COLUMN IF NOT EXISTS seq BIGINT NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN IF NOT EXISTS sender_type TEXT;
ALTER TABLE messages ADD COLUMN IF NOT EXISTS sender_id TEXT;
ALTER TABLE messages ADD COLUMN IF NOT EXISTS message_kind TEXT NOT NULL DEFAULT 'user_text';
ALTER TABLE messages ADD COLUMN IF NOT EXISTS visibility TEXT NOT NULL DEFAULT 'room';
ALTER TABLE messages ADD COLUMN IF NOT EXISTS parent_message_id UUID REFERENCES messages(id) ON DELETE SET NULL;
ALTER TABLE messages ADD COLUMN IF NOT EXISTS target_agent_ids JSONB NOT NULL DEFAULT '[]';
ALTER TABLE messages ADD COLUMN IF NOT EXISTS client_message_id UUID;

CREATE INDEX IF NOT EXISTS idx_messages_room_seq ON messages(room_id, seq);
CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_room_client_id
    ON messages(room_id, client_message_id)
    WHERE client_message_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS orchestration_runs (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    room_id            UUID NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    trigger_message_id UUID REFERENCES messages(id) ON DELETE SET NULL,
    pattern            TEXT,
    leader_agent_id    UUID REFERENCES agents(id) ON DELETE SET NULL,
    status             TEXT NOT NULL DEFAULT 'pending',
    graph_snapshot     JSONB NOT NULL DEFAULT '{}',
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at       TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_orchestration_runs_room ON orchestration_runs(room_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_orchestration_runs_status ON orchestration_runs(status);

ALTER TABLE cost_records ADD COLUMN IF NOT EXISTS room_id UUID REFERENCES rooms(id) ON DELETE SET NULL;
ALTER TABLE cost_records ADD COLUMN IF NOT EXISTS orchestration_run_id UUID REFERENCES orchestration_runs(id) ON DELETE SET NULL;
ALTER TABLE cost_records ADD COLUMN IF NOT EXISTS triggered_by_user_id TEXT;

CREATE INDEX IF NOT EXISTS idx_cost_records_room ON cost_records(room_id, created_at DESC);

ALTER TABLE governance_audit ADD COLUMN IF NOT EXISTS room_id UUID REFERENCES rooms(id) ON DELETE SET NULL;
ALTER TABLE governance_audit ADD COLUMN IF NOT EXISTS orchestration_run_id UUID REFERENCES orchestration_runs(id) ON DELETE SET NULL;
ALTER TABLE governance_audit ADD COLUMN IF NOT EXISTS triggered_by_user_id TEXT;

CREATE INDEX IF NOT EXISTS idx_governance_audit_room ON governance_audit(room_id, created_at DESC);

-- Backfill: existing conversations become direct rooms (id = conversation id).
INSERT INTO rooms (
    id,
    tenant_id,
    conversation_id,
    title,
    room_type,
    orchestration_mode,
    visibility,
    metadata,
    primary_agent_id,
    created_at,
    updated_at,
    last_message_at
)
SELECT
    c.id,
    COALESCE(c.metadata->>'tenant_id', 'default'),
    c.id,
    c.metadata->>'title',
    'direct',
    'single',
    'public',
    c.metadata,
    c.agent_id,
    c.started_at,
    c.started_at,
    c.last_message_at
FROM conversations c
WHERE NOT EXISTS (
    SELECT 1 FROM rooms r WHERE r.conversation_id = c.id
);

-- Backfill: primary agent as leader participant on direct rooms.
INSERT INTO room_participants (room_id, participant_type, participant_id, role, permissions)
SELECT
    r.id,
    'agent',
    r.primary_agent_id::text,
    'leader',
    '{}'::jsonb
FROM rooms r
WHERE r.primary_agent_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM room_participants rp
      WHERE rp.room_id = r.id
        AND rp.participant_type = 'agent'
        AND rp.participant_id = r.primary_agent_id::text
  );

-- Backfill: link messages to their room.
UPDATE messages m
SET room_id = m.conversation_id
WHERE m.room_id IS NULL
  AND EXISTS (SELECT 1 FROM rooms r WHERE r.id = m.conversation_id);
