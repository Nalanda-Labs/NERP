-- Quotes and invoices: built-in record types on the smart-process engine (their own
-- stages, products, client, documents, automation), which cannot be deleted. Quotes and
-- invoices made from a deal (or an invoice from a quote) point back to it.

-- 'quote' | 'invoice' for those built-in types; NULL for admin-defined smart processes.
ALTER TABLE crm_entity_types ADD COLUMN kind TEXT UNIQUE;

-- A smart process may already use one of these codes; it keeps its records and gets a new code.
UPDATE crm_entity_types SET code = code || '-' || id WHERE code IN ('quote', 'invoice') AND NOT is_system;

INSERT INTO crm_entity_types (id, code, name, name_plural, has_pipelines, has_stages, has_amount, has_client, is_system, icon, sort, kind)
VALUES (5, 'quote', 'Quote', 'Quotes', FALSE, TRUE, TRUE, TRUE, FALSE, 'file', 50, 'quote'),
       (6, 'invoice', 'Invoice', 'Invoices', FALSE, TRUE, TRUE, TRUE, FALSE, 'list', 51, 'invoice');
INSERT INTO crm_counters (entity_type) VALUES (5), (6);

INSERT INTO crm_categories (id, entity_type, name, sort, is_default)
VALUES ('00000000-0000-0000-0000-00000000c005', 5, 'Quotes', 10, TRUE),
       ('00000000-0000-0000-0000-00000000c006', 6, 'Invoices', 10, TRUE);
INSERT INTO crm_stages (category_id, name, color, sort, semantics) VALUES
    ('00000000-0000-0000-0000-00000000c005', 'Draft', '#94a3b8', 10, 'process'),
    ('00000000-0000-0000-0000-00000000c005', 'Sent', '#3b82f6', 20, 'process'),
    ('00000000-0000-0000-0000-00000000c005', 'Accepted', '#22c55e', 90, 'success'),
    ('00000000-0000-0000-0000-00000000c005', 'Declined', '#ef4444', 95, 'failure'),
    ('00000000-0000-0000-0000-00000000c006', 'Draft', '#94a3b8', 10, 'process'),
    ('00000000-0000-0000-0000-00000000c006', 'Sent', '#3b82f6', 20, 'process'),
    ('00000000-0000-0000-0000-00000000c006', 'Paid', '#22c55e', 90, 'success'),
    ('00000000-0000-0000-0000-00000000c006', 'Cancelled', '#ef4444', 95, 'failure');

-- Same access rights as deals to start with.
INSERT INTO crm_role_permissions (role_id, entity_type, action, scope)
SELECT role_id, 5, action, scope FROM crm_role_permissions WHERE entity_type = 2;
INSERT INTO crm_role_permissions (role_id, entity_type, action, scope)
SELECT role_id, 6, action, scope FROM crm_role_permissions WHERE entity_type = 2;

-- The built-in quote and invoice templates are offered for the new types too.
UPDATE crm_doc_templates SET entity_types = array_append(entity_types, 6::smallint)
 WHERE builtin_key LIKE '%.invoice%' AND NOT (6 = ANY(entity_types));
UPDATE crm_doc_templates SET entity_types = array_append(entity_types, 5::smallint)
 WHERE builtin_key = 'typst.quote' AND NOT (5 = ANY(entity_types));

-- "Made from this record" lists.
CREATE INDEX crm_items_source_idx ON crm_items (source_item_id ASC) WHERE source_item_id IS NOT NULL;
