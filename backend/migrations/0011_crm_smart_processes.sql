-- Smart processes: record types defined by CRM admins (e.g. Contracts, Shipments), on the
-- same item engine as deals. Built-in types keep ids 1–4; custom types start at 128.

ALTER TABLE crm_entity_types ADD COLUMN has_client BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE crm_entity_types ADD COLUMN icon TEXT NOT NULL DEFAULT 'folder';
ALTER TABLE crm_entity_types ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
-- Leads and deals are linked to a company and contacts ("the client").
UPDATE crm_entity_types SET has_client = TRUE WHERE id IN (1, 2);
UPDATE crm_entity_types SET icon = CASE id WHEN 1 THEN 'send' WHEN 2 THEN 'funnel' WHEN 3 THEN 'user' WHEN 4 THEN 'building' ELSE icon END;
