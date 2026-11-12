-- Credit notes against invoices in the books, and sales commissions from paid invoices.

-- ---- credit notes ----
ALTER TABLE acc_invoices ADD COLUMN credited NUMERIC(18, 2) NOT NULL DEFAULT 0;
ALTER TABLE acc_invoices DROP CONSTRAINT acc_invoices_status_check;
-- 'credited': settled by credit notes (and payments) rather than payments alone.
ALTER TABLE acc_invoices ADD CONSTRAINT acc_invoices_status_check CHECK (status IN ('open', 'paid', 'credited', 'void'));

CREATE TABLE acc_credit_notes (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    number        BIGINT NOT NULL UNIQUE,
    invoice_id    UUID NOT NULL REFERENCES acc_invoices (item_id),
    date          DATE NOT NULL,
    amount        NUMERIC(18, 2) NOT NULL CHECK (amount > 0),
    reason        TEXT,
    status        TEXT NOT NULL DEFAULT 'posted' CHECK (status IN ('posted', 'void')),
    entry_id      UUID NOT NULL REFERENCES acc_entries (id),
    created_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX acc_credit_notes_invoice_idx ON acc_credit_notes (invoice_id);
INSERT INTO acc_counters (kind, next) VALUES ('credit_note', 1) ON CONFLICT DO NOTHING;

-- ---- commissions ----
CREATE TABLE comm_plans (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL,
    is_active       BOOLEAN NOT NULL DEFAULT TRUE,
    -- Percent of the paid net sale (without tax), in the base currency.
    rate            NUMERIC(7, 4) NOT NULL DEFAULT 0 CHECK (rate >= 0 AND rate <= 100),
    -- Higher rates once a person's paid sales in the month pass a threshold:
    -- [{"from": "10000", "rate": "7.5"}, …], each replacing the rate from that point on.
    tiers           JSONB NOT NULL DEFAULT '[]',
    -- Only these products' lines count (empty: the whole invoice).
    product_ids     UUID[] NOT NULL DEFAULT '{}',
    -- Who earns under it (both empty: everyone who is responsible for invoices).
    user_ids        UUID[] NOT NULL DEFAULT '{}',
    department_ids  UUID[] NOT NULL DEFAULT '{}',
    created_by      UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Every change in what someone earned: earned (positive) or reversed (negative), in the month
-- it happened. What an invoice earned is the sum of its entries.
CREATE TABLE comm_entries (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    plan_id     UUID NOT NULL REFERENCES comm_plans (id) ON DELETE RESTRICT,
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    invoice_id  UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    month       DATE NOT NULL,
    -- The sale the commission is on (base currency) and the rate used.
    base        NUMERIC(18, 2) NOT NULL,
    rate        NUMERIC(7, 4) NOT NULL,
    amount      NUMERIC(18, 2) NOT NULL,
    reason      TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX comm_entries_user_idx ON comm_entries (user_id, month);
CREATE INDEX comm_entries_invoice_idx ON comm_entries (invoice_id, plan_id, user_id);

-- Months closed for payroll: new changes land in the next open month.
CREATE TABLE comm_months (
    month      DATE PRIMARY KEY,
    locked_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    locked_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'commissions.manage' FROM roles WHERE code = 'admin'
ON CONFLICT DO NOTHING;
