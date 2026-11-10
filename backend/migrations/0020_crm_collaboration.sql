-- Observers on CRM records, tasks linked to CRM records, and email templates.

-- Employees who follow a record: they can open it (even outside their access scope) and are
-- told about stage changes and comments.
CREATE TABLE crm_item_observers (
    item_id     UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    added_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (item_id, user_id)
);
CREATE INDEX crm_item_observers_user_idx ON crm_item_observers (user_id ASC);

-- Tasks about CRM records (a task may concern several).
CREATE TABLE task_crm_links (
    task_id     UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    item_id     UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (task_id, item_id)
);
CREATE INDEX task_crm_links_item_idx ON task_crm_links (item_id ASC);

-- Email templates with {{placeholders}}: shared (owner NULL, managed by CRM admins) or
-- personal. Empty entity_types means every record type.
CREATE TABLE crm_email_templates (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name          TEXT NOT NULL,
    subject       TEXT NOT NULL DEFAULT '',
    body          TEXT NOT NULL DEFAULT '',
    entity_types  SMALLINT[] NOT NULL DEFAULT '{}',
    owner_id      UUID REFERENCES users (id) ON DELETE CASCADE,
    created_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_email_templates_owner_idx ON crm_email_templates (owner_id ASC);
