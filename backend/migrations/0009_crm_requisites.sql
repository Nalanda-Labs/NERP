-- CRM requisites: legal details of companies and contacts (identifiers per country, legal
-- address, bank accounts), and which of them a deal or lead is billed to.

CREATE TABLE crm_requisites (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    item_id     UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    -- company: an organisation; person: an individual or sole trader.
    kind        TEXT NOT NULL DEFAULT 'company' CHECK (kind IN ('company', 'person')),
    country     CHAR(2) NOT NULL,
    -- Legal name as registered.
    name        TEXT NOT NULL,
    -- Identifier values by field code (see crm::requisites::schema).
    fields      JSONB NOT NULL DEFAULT '{}',
    address     JSONB,
    is_default  BOOLEAN NOT NULL DEFAULT FALSE,
    sort        INT NOT NULL DEFAULT 100,
    created_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_requisites_item_idx ON crm_requisites (item_id ASC, sort ASC);

CREATE TABLE crm_requisite_banks (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    requisite_id  UUID NOT NULL REFERENCES crm_requisites (id) ON DELETE CASCADE,
    name          TEXT NOT NULL DEFAULT '',
    fields        JSONB NOT NULL DEFAULT '{}',
    is_default    BOOLEAN NOT NULL DEFAULT FALSE,
    sort          INT NOT NULL DEFAULT 100
);
CREATE INDEX crm_requisite_banks_req_idx ON crm_requisite_banks (requisite_id ASC, sort ASC);

-- Compact identifier values ("DE123456789", "27AAPFU0939F1ZV") for duplicate detection
-- and search, range-sharded so it splits as it grows.
CREATE TABLE crm_requisite_ids (
    requisite_id  UUID NOT NULL REFERENCES crm_requisites (id) ON DELETE CASCADE,
    item_id       UUID NOT NULL,
    code          TEXT NOT NULL,
    value         TEXT NOT NULL,
    PRIMARY KEY (requisite_id, code)
);
CREATE INDEX crm_requisite_ids_value_idx ON crm_requisite_ids (value ASC);
CREATE INDEX crm_requisite_ids_item_idx ON crm_requisite_ids (item_id ASC);

-- The buyer details a deal or lead is invoiced to (a requisite of its company or contacts).
ALTER TABLE crm_items ADD COLUMN requisite_id UUID REFERENCES crm_requisites (id) ON DELETE SET NULL;
