-- Custom CRM fields seen only by chosen people and departments (empty: everyone).
-- CRM administrators always see every field.
ALTER TABLE crm_field_defs
    ADD COLUMN visible_user_ids uuid[] NOT NULL DEFAULT '{}',
    ADD COLUMN visible_department_ids uuid[] NOT NULL DEFAULT '{}';
