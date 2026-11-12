-- Stock movements posted to the books (src/inventory/costing.rs).
ALTER TABLE acc_entries DROP CONSTRAINT IF EXISTS acc_entries_source_check;
ALTER TABLE acc_entries ADD CONSTRAINT acc_entries_source_check CHECK (source IN ('manual', 'invoice', 'bill', 'payment', 'bank', 'opening', 'closing', 'inventory'));
