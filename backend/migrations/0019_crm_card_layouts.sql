-- Record card layouts: how a record type's fields are grouped into sections on its page and
-- in its form. One shared layout per type (set by CRM admins) and, optionally, a personal
-- one per employee that overrides it.
CREATE TABLE crm_card_layouts (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_type  SMALLINT NOT NULL REFERENCES crm_entity_types (id) ON DELETE CASCADE,
    -- NULL: the layout for everyone.
    user_id      UUID REFERENCES users (id) ON DELETE CASCADE,
    -- { "sections": [{ "id": "…", "title": "…", "fields": ["close_date", "uf.region", …] }],
    --   "hidden": ["probability", …], "hide_empty": false }
    layout       JSONB NOT NULL,
    updated_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX crm_card_layouts_common_idx ON crm_card_layouts (entity_type) WHERE user_id IS NULL;
CREATE UNIQUE INDEX crm_card_layouts_user_idx ON crm_card_layouts (entity_type, user_id) WHERE user_id IS NOT NULL;
