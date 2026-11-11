-- Sales generator: deals (or leads) made for every client in a segment, now or on a repeat
-- (repeat sales, renewals, seasonal offers).
CREATE TABLE mkt_generators (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name         TEXT NOT NULL,
    segment_id   UUID NOT NULL REFERENCES mkt_segments (id) ON DELETE CASCADE,
    -- The kind of record made: a deal (2) or a lead (1).
    target       SMALLINT NOT NULL CHECK (target IN (1, 2)),
    category_id  UUID REFERENCES crm_categories (id) ON DELETE SET NULL,
    stage_id     UUID REFERENCES crm_stages (id) ON DELETE SET NULL,
    -- With {{name}} (the client), {{month}} and {{year}}.
    title        TEXT NOT NULL,
    -- owner: the client's responsible person; users: the chosen people in turn.
    assign       TEXT NOT NULL DEFAULT 'owner' CHECK (assign IN ('owner', 'users')),
    user_ids     UUID[] NOT NULL DEFAULT '{}',
    next_user    INT NOT NULL DEFAULT 0,
    -- Skip clients who already have an open record of that kind (in that pipeline).
    skip_open    BOOLEAN NOT NULL DEFAULT TRUE,
    -- once | weekly | monthly | quarterly
    repeat       TEXT NOT NULL DEFAULT 'once' CHECK (repeat IN ('once', 'weekly', 'monthly', 'quarterly')),
    next_run_at  TIMESTAMPTZ,
    last_run_at  TIMESTAMPTZ,
    -- What the last run did: {"created": n, "skipped": n, "failed": n}.
    last_result  JSONB,
    is_active    BOOLEAN NOT NULL DEFAULT TRUE,
    created_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX mkt_generators_due_idx ON mkt_generators (next_run_at) WHERE is_active AND next_run_at IS NOT NULL;

-- Every record a generator made, and for which client.
CREATE TABLE mkt_generator_items (
    generator_id UUID NOT NULL REFERENCES mkt_generators (id) ON DELETE CASCADE,
    client_id    UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    item_id      UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX mkt_generator_items_idx ON mkt_generator_items (generator_id, created_at DESC);
