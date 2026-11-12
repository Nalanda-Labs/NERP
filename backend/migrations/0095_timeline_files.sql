-- Files attached to comments on a record's timeline (and attachments brought over by a
-- migration from another CRM). Whoever can see the record can download them.
ALTER TABLE crm_timeline ADD COLUMN file_ids UUID[] NOT NULL DEFAULT '{}';
CREATE INDEX crm_timeline_files ON crm_timeline USING gin (file_ids) WHERE file_ids <> '{}';
