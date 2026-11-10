-- Commerce: an online shop on top of the product catalog. Orders placed in the shop are
-- records of the built-in "order" type (stages, products, client, documents, automation),
-- linked to the customer's contact.

-- A smart process that already uses the code keeps its records under a new code.
UPDATE crm_entity_types SET code = code || '-' || id WHERE code IN ('order', 'orders') AND NOT is_system;
INSERT INTO crm_entity_types (id, code, name, name_plural, has_pipelines, has_stages, has_amount, has_client, is_system, icon, sort, kind)
VALUES (7, 'order', 'Order', 'Orders', FALSE, TRUE, TRUE, TRUE, FALSE, 'cart', 52, 'order');
INSERT INTO crm_counters (entity_type) VALUES (7);

INSERT INTO crm_categories (id, entity_type, name, sort, is_default)
VALUES ('00000000-0000-0000-0000-00000000c007', 7, 'Orders', 10, TRUE);
INSERT INTO crm_stages (category_id, name, color, sort, semantics) VALUES
    ('00000000-0000-0000-0000-00000000c007', 'New', '#94a3b8', 10, 'process'),
    ('00000000-0000-0000-0000-00000000c007', 'Paid', '#3b82f6', 20, 'process'),
    ('00000000-0000-0000-0000-00000000c007', 'Shipped', '#a855f7', 30, 'process'),
    ('00000000-0000-0000-0000-00000000c007', 'Completed', '#22c55e', 90, 'success'),
    ('00000000-0000-0000-0000-00000000c007', 'Cancelled', '#ef4444', 95, 'failure');

INSERT INTO crm_role_permissions (role_id, entity_type, action, scope)
SELECT role_id, 7, action, scope FROM crm_role_permissions WHERE entity_type = 2;

-- Invoices can be made from an order.
UPDATE crm_doc_templates SET entity_types = array_append(entity_types, 7::smallint)
 WHERE builtin_key LIKE '%.invoice%' AND NOT (7 = ANY(entity_types));

CREATE TABLE shop_categories (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT NOT NULL,
    slug       TEXT NOT NULL UNIQUE,
    sort       INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Shop fields of a product. stock NULL = not counted (always available).
ALTER TABLE crm_products ADD COLUMN in_shop BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE crm_products ADD COLUMN slug TEXT UNIQUE;
ALTER TABLE crm_products ADD COLUMN shop_category_id UUID REFERENCES shop_categories (id) ON DELETE SET NULL;
ALTER TABLE crm_products ADD COLUMN image_file_id UUID REFERENCES files (id) ON DELETE SET NULL;
ALTER TABLE crm_products ADD COLUMN stock NUMERIC(24, 4);
CREATE INDEX crm_products_shop_idx ON crm_products (shop_category_id ASC) WHERE in_shop;

-- One row: the shop's settings.
CREATE TABLE shop_settings (
    id                   BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    enabled              BOOLEAN NOT NULL DEFAULT FALSE,
    name                 TEXT NOT NULL DEFAULT 'Shop',
    intro                TEXT,
    currency             CHAR(3),
    handlers             UUID[] NOT NULL DEFAULT '{}',
    payment_instructions TEXT,
    terms                TEXT,
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO shop_settings (id) VALUES (TRUE);

-- What the customer entered at checkout, and the link to their order page.
CREATE TABLE shop_orders (
    item_id        UUID PRIMARY KEY REFERENCES crm_items (id) ON DELETE CASCADE,
    token          TEXT NOT NULL UNIQUE,
    name           TEXT NOT NULL,
    email          TEXT NOT NULL,
    phone          TEXT,
    address        TEXT NOT NULL,
    note           TEXT,
    -- Stock of a cancelled order goes back to the shelf once.
    stock_returned BOOLEAN NOT NULL DEFAULT FALSE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
