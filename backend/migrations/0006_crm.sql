-- CRM, phase 1.
--
-- Sized for millions of records on YugabyteDB:
--  * Secondary indexes lead with ASC columns, so they are range-sharded and can split as they
--    grow (YugabyteDB hash-shards on the first column by default, and with only a handful of
--    record types a hashed entity_type would pin each type to one unsplittable tablet).
--  * Search uses a trigram GIN index over a precomputed lowercase search column.
--
-- One generic "item" table holds every CRM record type (leads, deals, contacts,
-- companies, and later quotes, invoices and user-defined types). A record type is a row
-- in crm_entity_types; type-specific behaviour lives in code. Custom field values are
-- stored in a JSONB column validated against crm_field_defs.

-- ---------- Record types ----------
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE TABLE crm_entity_types (
    id            SMALLINT PRIMARY KEY,
    code          TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL,
    name_plural   TEXT NOT NULL,
    -- Feature switches per type.
    has_pipelines BOOLEAN NOT NULL DEFAULT FALSE,   -- several categories (pipelines)
    has_stages    BOOLEAN NOT NULL DEFAULT FALSE,
    has_amount    BOOLEAN NOT NULL DEFAULT FALSE,   -- opportunity + currency + products
    is_system     BOOLEAN NOT NULL DEFAULT TRUE,
    sort          INT NOT NULL DEFAULT 100
);
INSERT INTO crm_entity_types (id, code, name, name_plural, has_pipelines, has_stages, has_amount, sort) VALUES
    (1, 'lead',    'Lead',    'Leads',     FALSE, TRUE,  TRUE,  20),
    (2, 'deal',    'Deal',    'Deals',     TRUE,  TRUE,  TRUE,  10),
    (3, 'contact', 'Contact', 'Contacts',  FALSE, FALSE, FALSE, 30),
    (4, 'company', 'Company', 'Companies', FALSE, FALSE, FALSE, 40);

CREATE TABLE crm_counters (
    entity_type  SMALLINT PRIMARY KEY REFERENCES crm_entity_types (id),
    value        BIGINT NOT NULL DEFAULT 0
);
INSERT INTO crm_counters (entity_type) SELECT id FROM crm_entity_types;

-- ---------- Pipelines (categories) and stages ----------
CREATE TABLE crm_categories (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_type  SMALLINT NOT NULL REFERENCES crm_entity_types (id),
    name         TEXT NOT NULL,
    sort         INT NOT NULL DEFAULT 100,
    is_default   BOOLEAN NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_categories_type_idx ON crm_categories (entity_type ASC, sort ASC);

CREATE TABLE crm_stages (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    category_id  UUID NOT NULL REFERENCES crm_categories (id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    color        TEXT NOT NULL DEFAULT '#3b82f6',
    sort         INT NOT NULL DEFAULT 100,
    -- process: still open; success: won/converted; failure: lost/junk.
    semantics    TEXT NOT NULL DEFAULT 'process' CHECK (semantics IN ('process', 'success', 'failure'))
);
CREATE INDEX crm_stages_category_idx ON crm_stages (category_id ASC, sort ASC);

-- ---------- Currencies ----------
-- Enabled currencies with their rate to the base currency (1 unit = rate x base).
CREATE TABLE crm_currencies (
    code        CHAR(3) PRIMARY KEY,
    rate        NUMERIC(24, 10) NOT NULL DEFAULT 1 CHECK (rate > 0),
    is_base     BOOLEAN NOT NULL DEFAULT FALSE,
    sort        INT NOT NULL DEFAULT 100,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO crm_currencies (code, rate, is_base, sort) VALUES ('USD', 1, TRUE, 1);

-- ---------- Custom fields ----------
CREATE TABLE crm_field_defs (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_type  SMALLINT NOT NULL REFERENCES crm_entity_types (id),
    code         TEXT NOT NULL,      -- key inside crm_items.uf, e.g. "uf_contract_no"
    label        TEXT NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('string', 'text', 'number', 'money', 'date', 'datetime',
                                               'boolean', 'list', 'user', 'crm', 'url', 'file', 'address')),
    is_multiple  BOOLEAN NOT NULL DEFAULT FALSE,
    is_required  BOOLEAN NOT NULL DEFAULT FALSE,
    show_in_list BOOLEAN NOT NULL DEFAULT FALSE,
    -- list: {"options":[{"id":"a","label":"A"}]}; crm: {"types":["contact"]}
    settings     JSONB NOT NULL DEFAULT '{}'::jsonb,
    sort         INT NOT NULL DEFAULT 100,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX crm_field_defs_code_uidx ON crm_field_defs (entity_type, code);

-- ---------- Items ----------
CREATE TABLE crm_items (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_type       SMALLINT NOT NULL REFERENCES crm_entity_types (id),
    number            BIGINT NOT NULL,
    title             TEXT NOT NULL,
    category_id       UUID REFERENCES crm_categories (id),
    stage_id          UUID REFERENCES crm_stages (id),
    stage_semantics   TEXT NOT NULL DEFAULT 'process',
    assigned_to       UUID REFERENCES users (id) ON DELETE SET NULL,
    created_by        UUID REFERENCES users (id) ON DELETE SET NULL,
    updated_by        UUID REFERENCES users (id) ON DELETE SET NULL,
    -- People (contact, lead)
    honorific         TEXT,
    first_name        TEXT,
    middle_name       TEXT,
    last_name         TEXT,
    job_title         TEXT,
    birthdate         DATE,
    -- Organisations (company) and the company name typed on a lead
    company_name      TEXT,
    company_kind      TEXT,       -- customer, partner, supplier, competitor, other
    industry          TEXT,
    employees         TEXT,       -- size band, e.g. "50-250"
    annual_revenue    NUMERIC(24, 4),
    -- Money (lead, deal)
    opportunity       NUMERIC(24, 4) NOT NULL DEFAULT 0,
    currency          CHAR(3),
    opportunity_base  NUMERIC(24, 4) NOT NULL DEFAULT 0,   -- converted to the base currency, for reports
    is_manual_amount  BOOLEAN NOT NULL DEFAULT TRUE,       -- false: amount = sum of product rows
    probability       SMALLINT CHECK (probability BETWEEN 0 AND 100),
    close_date        DATE,
    -- Common
    source            TEXT,       -- call, email, web, referral, advertising, partner, exhibition, other
    source_details    TEXT,
    address           JSONB,      -- {line1,line2,city,region,postal_code,country}
    comments          TEXT,
    company_id        UUID REFERENCES crm_items (id) ON DELETE SET NULL,   -- primary company link
    source_item_id    UUID REFERENCES crm_items (id) ON DELETE SET NULL,   -- e.g. the lead it was converted from
    uf                JSONB NOT NULL DEFAULT '{}'::jsonb,
    is_closed         BOOLEAN NOT NULL DEFAULT FALSE,
    closed_at         TIMESTAMPTZ,
    stage_changed_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_activity_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Lowercased text of title, names, emails, phones and company for search.
    search_text       TEXT NOT NULL DEFAULT '',
    position          DOUBLE PRECISION NOT NULL DEFAULT 0,
    deleted_at        TIMESTAMPTZ,
    deleted_by        UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX crm_items_number_uidx ON crm_items (entity_type ASC, number DESC);
-- Default list order (most recently changed first) and per-stage boards.
CREATE INDEX crm_items_updated_idx ON crm_items (entity_type ASC, updated_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX crm_items_stage_idx ON crm_items (entity_type ASC, category_id ASC, stage_id ASC, position ASC) WHERE deleted_at IS NULL;
CREATE INDEX crm_items_assigned_idx ON crm_items (entity_type ASC, assigned_to ASC, updated_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX crm_items_company_idx ON crm_items (company_id ASC) WHERE company_id IS NOT NULL;
CREATE INDEX crm_items_source_item_idx ON crm_items (source_item_id ASC) WHERE source_item_id IS NOT NULL;
CREATE INDEX crm_items_deleted_idx ON crm_items (deleted_at ASC) WHERE deleted_at IS NOT NULL;
CREATE INDEX crm_items_search_idx ON crm_items USING ybgin (search_text gin_trgm_ops);

-- Contacts linked to a deal, lead or company (many-to-many).
CREATE TABLE crm_item_contacts (
    item_id     UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    contact_id  UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    is_primary  BOOLEAN NOT NULL DEFAULT FALSE,
    sort        INT NOT NULL DEFAULT 0,
    PRIMARY KEY (item_id, contact_id)
);
CREATE INDEX crm_item_contacts_contact_idx ON crm_item_contacts (contact_id ASC);

-- Phones, emails, websites and messengers.
CREATE TABLE crm_multifields (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    item_id     UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    kind        TEXT NOT NULL CHECK (kind IN ('phone', 'email', 'web', 'im')),
    value_type  TEXT NOT NULL DEFAULT 'work',
    value       TEXT NOT NULL,
    normalized  TEXT NOT NULL,       -- digits-only phone, lowercased email/url, for matching
    sort        INT NOT NULL DEFAULT 0
);
CREATE INDEX crm_multifields_item_idx ON crm_multifields (item_id ASC);
-- Exact-match lookups for duplicate detection ("is this phone/email already in the CRM?").
CREATE INDEX crm_multifields_match_idx ON crm_multifields (kind ASC, normalized ASC);

-- ---------- Timeline ----------
CREATE TABLE crm_timeline (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    item_id     UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    -- comment, created, stage, field, converted, merged, restored, activity, products
    kind        TEXT NOT NULL,
    author_id   UUID REFERENCES users (id) ON DELETE SET NULL,
    body        TEXT,
    data        JSONB NOT NULL DEFAULT '{}'::jsonb,
    is_pinned   BOOLEAN NOT NULL DEFAULT FALSE,
    edited_at   TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_timeline_item_idx ON crm_timeline (item_id ASC, created_at DESC);

-- ---------- Activities ----------
CREATE TABLE crm_activities (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    item_id         UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    kind            TEXT NOT NULL CHECK (kind IN ('todo', 'call', 'meeting', 'email')),
    subject         TEXT NOT NULL,
    description     TEXT,
    direction       TEXT CHECK (direction IN ('incoming', 'outgoing')),
    starts_at       TIMESTAMPTZ,
    ends_at         TIMESTAMPTZ,
    deadline        TIMESTAMPTZ,
    responsible_id  UUID NOT NULL REFERENCES users (id),
    created_by      UUID REFERENCES users (id) ON DELETE SET NULL,
    is_completed    BOOLEAN NOT NULL DEFAULT FALSE,
    completed_at    TIMESTAMPTZ,
    result          TEXT,
    remind_before_minutes INT,
    reminded_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_activities_item_idx ON crm_activities (item_id ASC, is_completed ASC, deadline ASC);
CREATE INDEX crm_activities_responsible_idx ON crm_activities (responsible_id ASC, is_completed ASC, deadline ASC);
CREATE INDEX crm_activities_reminder_idx ON crm_activities (deadline ASC) WHERE NOT is_completed AND reminded_at IS NULL;

-- ---------- Products ----------
CREATE TABLE crm_products (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name          TEXT NOT NULL,
    sku           TEXT,
    description   TEXT,
    price         NUMERIC(24, 4) NOT NULL DEFAULT 0,
    currency      CHAR(3) NOT NULL,
    tax_rate      NUMERIC(7, 4),          -- percent, e.g. 18.0000
    tax_included  BOOLEAN NOT NULL DEFAULT FALSE,
    unit          TEXT NOT NULL DEFAULT 'pcs',
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_products_name_idx ON crm_products USING ybgin (lower(name) gin_trgm_ops);

CREATE TABLE crm_item_products (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    item_id         UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    product_id      UUID REFERENCES crm_products (id) ON DELETE SET NULL,
    name            TEXT NOT NULL,
    price           NUMERIC(24, 4) NOT NULL,        -- unit price in the item's currency, before discount
    quantity        NUMERIC(18, 4) NOT NULL DEFAULT 1,
    unit            TEXT NOT NULL DEFAULT 'pcs',
    discount_kind   TEXT NOT NULL DEFAULT 'percent' CHECK (discount_kind IN ('percent', 'amount')),
    discount_value  NUMERIC(24, 4) NOT NULL DEFAULT 0,
    tax_rate        NUMERIC(7, 4),
    tax_included    BOOLEAN NOT NULL DEFAULT FALSE,
    sort            INT NOT NULL DEFAULT 0
);
CREATE INDEX crm_item_products_item_idx ON crm_item_products (item_id ASC, sort ASC);

-- ---------- Access rights ----------
-- scope: none | own | department | subdepartments | all
CREATE TABLE crm_role_permissions (
    role_id      UUID NOT NULL REFERENCES roles (id) ON DELETE CASCADE,
    entity_type  SMALLINT NOT NULL REFERENCES crm_entity_types (id),
    action       TEXT NOT NULL CHECK (action IN ('read', 'add', 'write', 'delete', 'export', 'import')),
    scope        TEXT NOT NULL CHECK (scope IN ('none', 'own', 'department', 'subdepartments', 'all')),
    PRIMARY KEY (role_id, entity_type, action)
);

-- Default for the Employee role: see everything, create anything, change and delete own records.
INSERT INTO crm_role_permissions (role_id, entity_type, action, scope)
SELECT r.id, t.id, a.action, a.scope
FROM roles r CROSS JOIN crm_entity_types t
CROSS JOIN (VALUES ('read', 'all'), ('add', 'all'), ('write', 'own'), ('delete', 'own'),
                   ('export', 'own'), ('import', 'none')) AS a(action, scope)
WHERE r.code = 'employee';

-- ---------- Saved filters ----------
CREATE TABLE crm_saved_filters (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    entity_type  SMALLINT NOT NULL REFERENCES crm_entity_types (id),
    name         TEXT NOT NULL,
    filter       JSONB NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_saved_filters_user_idx ON crm_saved_filters (user_id ASC, entity_type ASC);

-- ---------- Default pipelines ----------
INSERT INTO crm_categories (id, entity_type, name, sort, is_default) VALUES
    ('00000000-0000-0000-0000-00000000c001', 1, 'Leads', 1, TRUE),
    ('00000000-0000-0000-0000-00000000c002', 2, 'Sales', 1, TRUE);

INSERT INTO crm_stages (category_id, name, color, sort, semantics) VALUES
    ('00000000-0000-0000-0000-00000000c001', 'New',           '#60a5fa', 10, 'process'),
    ('00000000-0000-0000-0000-00000000c001', 'In progress',   '#38bdf8', 20, 'process'),
    ('00000000-0000-0000-0000-00000000c001', 'Qualified',     '#a78bfa', 30, 'process'),
    ('00000000-0000-0000-0000-00000000c001', 'Converted',     '#22c55e', 90, 'success'),
    ('00000000-0000-0000-0000-00000000c001', 'Junk',          '#ef4444', 95, 'failure'),
    ('00000000-0000-0000-0000-00000000c002', 'New',           '#60a5fa', 10, 'process'),
    ('00000000-0000-0000-0000-00000000c002', 'Preparation',   '#38bdf8', 20, 'process'),
    ('00000000-0000-0000-0000-00000000c002', 'Proposal sent', '#a78bfa', 30, 'process'),
    ('00000000-0000-0000-0000-00000000c002', 'Negotiation',   '#f59e0b', 40, 'process'),
    ('00000000-0000-0000-0000-00000000c002', 'Won',           '#22c55e', 90, 'success'),
    ('00000000-0000-0000-0000-00000000c002', 'Lost',          '#ef4444', 95, 'failure');
