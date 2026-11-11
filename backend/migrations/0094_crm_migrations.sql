-- Moving over from another CRM (src/crm/migrate.rs): each run, and the old system's ids of
-- what it brought over (so links resolve and a second run updates instead of duplicating).
CREATE TABLE crm_migrations (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source      TEXT NOT NULL CHECK (source IN ('hubspot', 'salesforce', 'pipedrive', 'zoho', 'other')),
    plan        JSONB NOT NULL,
    status      TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'done', 'failed', 'undone')),
    progress    JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    undone_at   TIMESTAMPTZ
);

CREATE TABLE crm_migration_ids (
    source       TEXT NOT NULL,
    -- company, contact, lead, deal (crm_items), activity (crm_activities), note (crm_timeline)
    kind         TEXT NOT NULL,
    source_id    TEXT NOT NULL,
    item_id      UUID NOT NULL,
    migration_id UUID NOT NULL REFERENCES crm_migrations (id) ON DELETE CASCADE,
    -- made by the migration (undo removes it), or an existing record it matched
    created      BOOLEAN NOT NULL,
    PRIMARY KEY (source, kind, source_id)
);
CREATE INDEX crm_migration_ids_migration ON crm_migration_ids (migration_id);
