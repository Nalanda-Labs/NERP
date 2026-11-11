-- Catalog sections (the product categories) can name their own editors: people and
-- departments who manage the products in that section without the catalog-wide permission.
CREATE TABLE catalog_section_editors (
    category_id   UUID NOT NULL REFERENCES shop_categories (id) ON DELETE CASCADE,
    user_id       UUID REFERENCES users (id) ON DELETE CASCADE,
    department_id UUID REFERENCES departments (id) ON DELETE CASCADE,
    CHECK ((user_id IS NULL) <> (department_id IS NULL))
);
CREATE INDEX catalog_section_editors_idx ON catalog_section_editors (category_id);

-- Document templates can be limited to chosen people and departments (empty: everyone who
-- may make documents for the record).
ALTER TABLE crm_doc_templates ADD COLUMN user_ids UUID[] NOT NULL DEFAULT '{}';
ALTER TABLE crm_doc_templates ADD COLUMN department_ids UUID[] NOT NULL DEFAULT '{}';

-- Traffic sources of the company's own, beside the built-in ones.
CREATE TABLE crm_traffic_sources (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code       TEXT NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    -- utm_source values that mean this source (lower case).
    utm        TEXT[] NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A CRM record's own chat: its responsible person, creator and observers.
ALTER TABLE im_chats ADD COLUMN crm_item_id UUID UNIQUE REFERENCES crm_items (id) ON DELETE CASCADE;
