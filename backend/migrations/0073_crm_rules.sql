-- CRM: fields that must be filled before a record enters a stage, and services (products
-- that are not kept in stock).
ALTER TABLE crm_stages ADD COLUMN required_fields TEXT[] NOT NULL DEFAULT '{}';
ALTER TABLE crm_products ADD COLUMN is_service BOOLEAN NOT NULL DEFAULT FALSE;

-- Imported segments: a fixed list of records instead of a filter.
ALTER TABLE mkt_segments ADD COLUMN item_ids UUID[];
