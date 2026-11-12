-- Inventory costing (src/inventory/costing.rs): every movement carries its cost (in the base
-- currency), receipts make cost layers (FIFO) and each warehouse keeps a moving average; the
-- chosen method decides what goods leaving cost. Optionally each movement is posted to the
-- books (inventory against cost of goods sold, purchases or stock adjustments).
CREATE TABLE inv_settings (
    id                INT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    cost_method       TEXT NOT NULL DEFAULT 'average' CHECK (cost_method IN ('fifo', 'average')),
    post_to_books     BOOLEAN NOT NULL DEFAULT FALSE,
    inventory_account UUID REFERENCES acc_accounts (id),
    cogs_account      UUID REFERENCES acc_accounts (id),
    -- Credited when goods are received (purchases / goods received not invoiced).
    receipt_account   UUID REFERENCES acc_accounts (id),
    -- Write-offs and stock count differences.
    adjust_account    UUID REFERENCES acc_accounts (id)
);
INSERT INTO inv_settings (id) VALUES (1);

ALTER TABLE inv_stock ADD COLUMN avg_cost NUMERIC(24, 6) NOT NULL DEFAULT 0;

ALTER TABLE inv_movements ADD COLUMN unit_cost NUMERIC(24, 6);
-- Signed like the quantity: goods in > 0, goods out < 0.
ALTER TABLE inv_movements ADD COLUMN cost NUMERIC(24, 4);
ALTER TABLE inv_movements ADD COLUMN entry_id UUID;

CREATE TABLE inv_cost_layers (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    product_id   UUID NOT NULL REFERENCES crm_products (id) ON DELETE CASCADE,
    warehouse_id UUID NOT NULL REFERENCES inv_warehouses (id) ON DELETE CASCADE,
    movement_id  UUID REFERENCES inv_movements (id) ON DELETE SET NULL,
    received_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    quantity     NUMERIC(24, 4) NOT NULL,
    remaining    NUMERIC(24, 4) NOT NULL,
    unit_cost    NUMERIC(24, 6) NOT NULL
);
CREATE INDEX inv_cost_layers_open ON inv_cost_layers (product_id, warehouse_id, received_at) WHERE remaining > 0;

-- What is in stock now: one opening layer at the product's cost (its own, else the average
-- price it was received at, else nothing known: 0), in the base currency.
WITH base AS (SELECT code FROM crm_currencies WHERE is_base LIMIT 1),
known AS (
    SELECT p.id,
           COALESCE(
               p.cost * COALESCE((SELECT rate FROM crm_currencies c WHERE c.code = p.currency), 1),
               (SELECT SUM(l.price * l.quantity) / NULLIF(SUM(l.quantity), 0)
                       * COALESCE((SELECT rate FROM crm_currencies c WHERE c.code = COALESCE(d.currency, (SELECT code FROM base))), 1)
                FROM inv_document_lines l JOIN inv_documents d ON d.id = l.document_id
                WHERE l.product_id = p.id AND d.kind = 'receipt' AND d.status = 'posted' AND l.price IS NOT NULL AND l.quantity > 0
                GROUP BY d.currency ORDER BY SUM(l.quantity) DESC LIMIT 1),
               0) AS unit
    FROM crm_products p
)
UPDATE inv_stock s SET avg_cost = k.unit FROM known k WHERE k.id = s.product_id;
INSERT INTO inv_cost_layers (product_id, warehouse_id, quantity, remaining, unit_cost)
SELECT product_id, warehouse_id, quantity, quantity, avg_cost FROM inv_stock WHERE quantity > 0;
