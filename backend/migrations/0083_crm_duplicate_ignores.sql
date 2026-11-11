-- Groups of look-alike records someone marked "not duplicates": the group (by what they share)
-- stays hidden from the duplicate search until another record joins it.
CREATE TABLE crm_duplicate_ignores (
    entity_type  SMALLINT NOT NULL REFERENCES crm_entity_types (id) ON DELETE CASCADE,
    -- phone | email | name
    kind         TEXT NOT NULL CHECK (kind IN ('phone', 'email', 'name')),
    value        TEXT NOT NULL,
    item_ids     UUID[] NOT NULL,
    created_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (entity_type, kind, value)
);
CREATE INDEX crm_items_dup_name_idx ON crm_items (entity_type, lower(btrim(title))) WHERE deleted_at IS NULL;
