-- Recurring records: a deal, invoice or smart-process record copied on a schedule
-- (see crm::recurring). The rule has the same shape as recurring task templates.
CREATE TABLE crm_recurring (
    item_id     UUID PRIMARY KEY REFERENCES crm_items (id) ON DELETE CASCADE,
    rule        JSONB NOT NULL,
    -- The copy's date (due date, close date…) this many days after it is made; NULL = none.
    due_days    INT,
    next_run_at TIMESTAMPTZ,
    last_run_at TIMESTAMPTZ,
    run_count   INT NOT NULL DEFAULT 0,
    is_active   BOOLEAN NOT NULL DEFAULT TRUE,
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_recurring_due_idx ON crm_recurring (next_run_at ASC) WHERE is_active;
