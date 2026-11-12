-- Ledger entries can come from credit notes.
ALTER TABLE acc_entries DROP CONSTRAINT acc_entries_source_check;
ALTER TABLE acc_entries ADD CONSTRAINT acc_entries_source_check
    CHECK (source IN ('manual', 'invoice', 'bill', 'payment', 'bank', 'opening', 'closing', 'inventory', 'credit_note'));
