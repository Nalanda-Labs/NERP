-- Product catalog depth: properties, variants and price types.

-- Properties products can have (Material, Weight, Size…). Variant properties (list kind)
-- make variants: one product per combination of their values.
CREATE TABLE crm_product_props (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT NOT NULL,
    -- text | number | list
    kind       TEXT NOT NULL CHECK (kind IN ('text', 'number', 'list')),
    options    TEXT[] NOT NULL DEFAULT '{}',
    is_variant BOOLEAN NOT NULL DEFAULT FALSE,
    -- Shown on the shop's product page.
    in_shop    BOOLEAN NOT NULL DEFAULT TRUE,
    sort       INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A variant is a product of its own (SKU, price, stock) under its parent.
ALTER TABLE crm_products ADD COLUMN parent_id UUID REFERENCES crm_products (id) ON DELETE CASCADE;
-- Property values: property id → value.
ALTER TABLE crm_products ADD COLUMN props JSONB NOT NULL DEFAULT '{}';
CREATE INDEX crm_products_parent_idx ON crm_products (parent_id ASC) WHERE parent_id IS NOT NULL;

-- Extra prices (Wholesale, Partner…) beside a product's base price.
CREATE TABLE crm_price_types (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT NOT NULL,
    sort       INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE crm_product_prices (
    product_id    UUID NOT NULL REFERENCES crm_products (id) ON DELETE CASCADE,
    price_type_id UUID NOT NULL REFERENCES crm_price_types (id) ON DELETE CASCADE,
    price         NUMERIC(24, 4) NOT NULL CHECK (price >= 0),
    PRIMARY KEY (product_id, price_type_id)
);
