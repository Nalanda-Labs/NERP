-- Business processes on CRM records too: a process belongs to a list or to a CRM record
-- type, and runs on a list item or a CRM record.
ALTER TABLE wf_templates ALTER COLUMN list_id DROP NOT NULL;
ALTER TABLE wf_templates ADD COLUMN crm_entity_type SMALLINT REFERENCES crm_entity_types (id) ON DELETE CASCADE;
ALTER TABLE wf_templates ADD CONSTRAINT wf_templates_scope CHECK ((list_id IS NULL) <> (crm_entity_type IS NULL));
ALTER TABLE wf_instances ALTER COLUMN item_id DROP NOT NULL;
ALTER TABLE wf_instances ADD COLUMN crm_item_id UUID REFERENCES crm_items (id) ON DELETE CASCADE;
ALTER TABLE wf_instances ADD CONSTRAINT wf_instances_subject CHECK ((item_id IS NULL) <> (crm_item_id IS NULL));
CREATE INDEX wf_instances_crm_idx ON wf_instances (crm_item_id) WHERE crm_item_id IS NOT NULL;
CREATE INDEX wf_templates_crm_idx ON wf_templates (crm_entity_type) WHERE crm_entity_type IS NOT NULL;
