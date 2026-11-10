-- CRM analytics: stage history (for the funnel), sales targets, and indexes for reports.

-- Every stage a record entered, in order. The funnel counts how far records got.
CREATE TABLE crm_stage_history (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    item_id      UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    category_id  UUID NOT NULL REFERENCES crm_categories (id) ON DELETE CASCADE,
    stage_id     UUID NOT NULL REFERENCES crm_stages (id) ON DELETE CASCADE,
    semantics    TEXT NOT NULL,
    entered_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_stage_history_item_idx ON crm_stage_history (item_id ASC, entered_at ASC);

-- Backfill from the timeline's stage entries ...
INSERT INTO crm_stage_history (item_id, category_id, stage_id, semantics, entered_at)
SELECT x.item_id, s.category_id, s.id, s.semantics, x.created_at
  FROM (SELECT t.item_id, t.created_at,
               CASE WHEN t.data->'to'->>'id' ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                    THEN (t.data->'to'->>'id')::uuid END AS stage_id
          FROM crm_timeline t WHERE t.kind = 'stage') x
  JOIN crm_stages s ON s.id = x.stage_id;

-- ... and the current stage of records with no entry yet.
INSERT INTO crm_stage_history (item_id, category_id, stage_id, semantics, entered_at)
SELECT i.id, i.category_id, i.stage_id, i.stage_semantics, i.created_at
  FROM crm_items i
 WHERE i.stage_id IS NOT NULL AND i.category_id IS NOT NULL
   AND NOT EXISTS (SELECT 1 FROM crm_stage_history h WHERE h.item_id = i.id AND h.stage_id = i.stage_id);

-- Monthly sales targets in the base currency: per manager, or for the whole team (user_id NULL).
CREATE TABLE crm_sales_targets (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_type  SMALLINT NOT NULL REFERENCES crm_entity_types (id) ON DELETE CASCADE,
    user_id      UUID REFERENCES users (id) ON DELETE CASCADE,
    month        DATE NOT NULL CHECK (EXTRACT(DAY FROM month) = 1),
    amount       NUMERIC(24, 4) NOT NULL CHECK (amount >= 0),
    updated_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_sales_targets_month_idx ON crm_sales_targets (entity_type ASC, month ASC);

-- Reports filter records by when they were created or closed.
CREATE INDEX crm_items_created_idx ON crm_items (entity_type ASC, created_at ASC) WHERE deleted_at IS NULL;
CREATE INDEX crm_items_closed_idx ON crm_items (entity_type ASC, closed_at ASC) WHERE deleted_at IS NULL AND is_closed;
