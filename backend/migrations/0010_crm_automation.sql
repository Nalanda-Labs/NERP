-- CRM automation: robots (actions that run when a record enters a stage, optionally after a
-- delay and only if conditions hold) and triggers (events that move a record to a stage).

-- A built-in, hidden user that automated changes are attributed to ("Automation" in the
-- timeline). It can never sign in: it is inactive and its password hash matches nothing.
ALTER TABLE users ADD COLUMN is_system BOOLEAN NOT NULL DEFAULT FALSE;
INSERT INTO users (id, email, password_hash, first_name, last_name, is_active, is_system)
VALUES ('00000000-0000-0000-0000-0000000a0001', 'automation@nerp.invalid', '!', 'Automation', '', FALSE, TRUE)
ON CONFLICT DO NOTHING;

CREATE TABLE crm_robots (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_type    SMALLINT NOT NULL REFERENCES crm_entity_types (id),
    category_id    UUID NOT NULL REFERENCES crm_categories (id) ON DELETE CASCADE,
    stage_id       UUID NOT NULL REFERENCES crm_stages (id) ON DELETE CASCADE,
    name           TEXT NOT NULL,
    -- activity | task | notify | email | set_field | assign | move_stage | document | comment
    action         TEXT NOT NULL,
    params         JSONB NOT NULL DEFAULT '{}',
    delay_minutes  INT NOT NULL DEFAULT 0 CHECK (delay_minutes BETWEEN 0 AND 525600),
    -- [{ "field": "opportunity", "op": "gt", "value": "1000" }, …], all must hold.
    conditions     JSONB NOT NULL DEFAULT '[]',
    sort           INT NOT NULL DEFAULT 100,
    is_active      BOOLEAN NOT NULL DEFAULT TRUE,
    created_by     UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_robots_stage_idx ON crm_robots (stage_id ASC, sort ASC);
CREATE INDEX crm_robots_category_idx ON crm_robots (category_id ASC);

CREATE TABLE crm_triggers (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_type      SMALLINT NOT NULL REFERENCES crm_entity_types (id),
    category_id      UUID NOT NULL REFERENCES crm_categories (id) ON DELETE CASCADE,
    -- The stage the record moves to.
    stage_id         UUID NOT NULL REFERENCES crm_stages (id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    -- activity_done | comment_added | document_created | webhook
    event            TEXT NOT NULL,
    params           JSONB NOT NULL DEFAULT '{}',
    conditions       JSONB NOT NULL DEFAULT '[]',
    -- By default a trigger only moves records forward through the pipeline.
    allow_backwards  BOOLEAN NOT NULL DEFAULT FALSE,
    -- Secret for incoming webhooks.
    token            TEXT UNIQUE,
    sort             INT NOT NULL DEFAULT 100,
    is_active        BOOLEAN NOT NULL DEFAULT TRUE,
    created_by       UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_triggers_category_idx ON crm_triggers (category_id ASC, event ASC);

-- Each scheduled or executed robot, for delays, the record's automation log and debugging.
CREATE TABLE crm_robot_runs (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    robot_id     UUID REFERENCES crm_robots (id) ON DELETE SET NULL,
    robot_name   TEXT NOT NULL,
    item_id      UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    stage_id     UUID NOT NULL,
    -- scheduled | done | failed | skipped
    status       TEXT NOT NULL DEFAULT 'scheduled',
    run_at       TIMESTAMPTZ NOT NULL,
    finished_at  TIMESTAMPTZ,
    result       TEXT,
    depth        INT NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_robot_runs_item_idx ON crm_robot_runs (item_id ASC, created_at DESC);
