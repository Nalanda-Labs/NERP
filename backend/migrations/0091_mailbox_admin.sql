-- Mailboxes shared by role and by department (with sub-departments), besides chosen people.
ALTER TABLE crm_mailboxes ADD COLUMN shared_roles UUID[] NOT NULL DEFAULT '{}';
ALTER TABLE crm_mailboxes ADD COLUMN shared_departments UUID[] NOT NULL DEFAULT '{}';
