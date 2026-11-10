-- Inventory: warehouses, stock per warehouse, stock documents (receipts, write-offs,
-- transfers, stock counts) and the movement history behind every change.
--
-- crm_products.stock stays as the product's total over all warehouses (NULL: stock is not
-- counted for it), kept in step by every movement.

CREATE TABLE inv_warehouses (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT NOT NULL,
    address    TEXT,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    is_active  BOOLEAN NOT NULL DEFAULT TRUE,
    sort       INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX inv_warehouses_one_default ON inv_warehouses (is_default) WHERE is_default;

CREATE TABLE inv_stock (
    product_id   UUID NOT NULL REFERENCES crm_products (id) ON DELETE CASCADE,
    warehouse_id UUID NOT NULL REFERENCES inv_warehouses (id) ON DELETE CASCADE,
    quantity     NUMERIC(24, 4) NOT NULL DEFAULT 0,
    PRIMARY KEY (product_id, warehouse_id)
);
CREATE INDEX inv_stock_warehouse_idx ON inv_stock (warehouse_id ASC);

CREATE TABLE inv_documents (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- receipt | writeoff | transfer | count
    kind         TEXT NOT NULL CHECK (kind IN ('receipt', 'writeoff', 'transfer', 'count')),
    number       TEXT NOT NULL UNIQUE,
    -- draft | posted | cancelled
    status       TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'posted', 'cancelled')),
    -- Where goods come from (write-off, transfer) and go to (receipt, transfer, count).
    from_warehouse UUID REFERENCES inv_warehouses (id),
    to_warehouse   UUID REFERENCES inv_warehouses (id),
    -- Receipts: the supplier (a CRM company) and their invoice number.
    supplier_id  UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    reference    TEXT,
    currency     CHAR(3),
    comment      TEXT,
    doc_date     DATE NOT NULL DEFAULT CURRENT_DATE,
    created_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    posted_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    posted_at    TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX inv_documents_date_idx ON inv_documents (doc_date DESC, created_at DESC);

CREATE TABLE inv_document_lines (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    document_id UUID NOT NULL REFERENCES inv_documents (id) ON DELETE CASCADE,
    product_id  UUID NOT NULL REFERENCES crm_products (id),
    -- Receipts, write-offs, transfers: how many. Counts: how many were found.
    quantity    NUMERIC(24, 4) NOT NULL,
    -- Receipts: purchase price per unit.
    price       NUMERIC(24, 4),
    -- Counts: the change posting made (found − what the warehouse held), undone on cancel.
    applied     NUMERIC(24, 4),
    sort        INT NOT NULL DEFAULT 0
);
CREATE INDEX inv_document_lines_doc_idx ON inv_document_lines (document_id ASC);
CREATE INDEX inv_document_lines_product_idx ON inv_document_lines (product_id ASC);

-- Every change to stock, newest first per product.
CREATE TABLE inv_movements (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    product_id   UUID NOT NULL REFERENCES crm_products (id) ON DELETE CASCADE,
    warehouse_id UUID NOT NULL REFERENCES inv_warehouses (id) ON DELETE CASCADE,
    delta        NUMERIC(24, 4) NOT NULL,
    -- Quantity in that warehouse after the change.
    balance      NUMERIC(24, 4) NOT NULL,
    -- opening | document | document_cancelled | order | order_cancelled | order_reopened | product_form
    reason       TEXT NOT NULL,
    document_id  UUID REFERENCES inv_documents (id) ON DELETE SET NULL,
    item_id      UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    user_id      UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX inv_movements_product_idx ON inv_movements (product_id ASC, created_at DESC);

-- Document numbers per kind: RC-0001, WO-0001, TR-0001, IC-0001.
CREATE TABLE inv_counters (
    kind TEXT PRIMARY KEY,
    last BIGINT NOT NULL DEFAULT 0
);
INSERT INTO inv_counters (kind) VALUES ('receipt'), ('writeoff'), ('transfer'), ('count');

-- The shop sells from one warehouse (NULL: the default one); each order remembers which.
ALTER TABLE shop_settings ADD COLUMN warehouse_id UUID REFERENCES inv_warehouses (id) ON DELETE SET NULL;
ALTER TABLE shop_orders ADD COLUMN warehouse_id UUID REFERENCES inv_warehouses (id) ON DELETE SET NULL;

-- A first warehouse holding the stock counted so far.
INSERT INTO inv_warehouses (id, name, is_default, sort) VALUES ('00000000-0000-0000-0000-0000000000a1', 'Main warehouse', TRUE, 10);
INSERT INTO inv_stock (product_id, warehouse_id, quantity)
SELECT id, '00000000-0000-0000-0000-0000000000a1', stock FROM crm_products WHERE stock IS NOT NULL;
INSERT INTO inv_movements (product_id, warehouse_id, delta, balance, reason)
SELECT id, '00000000-0000-0000-0000-0000000000a1', stock, stock, 'opening' FROM crm_products WHERE stock IS NOT NULL AND stock <> 0;
UPDATE shop_orders SET warehouse_id = '00000000-0000-0000-0000-0000000000a1';
