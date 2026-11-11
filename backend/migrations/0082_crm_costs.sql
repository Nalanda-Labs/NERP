-- Gross profit: a product's cost price (in its currency), and the unit cost a deal's product
-- row carries (in the deal's currency, copied when the row is saved so later changes to the
-- product don't rewrite past deals). NULL: unknown.
ALTER TABLE crm_products ADD COLUMN cost NUMERIC(24, 4) CHECK (cost >= 0);
ALTER TABLE crm_item_products ADD COLUMN cost NUMERIC(24, 4) CHECK (cost >= 0);
