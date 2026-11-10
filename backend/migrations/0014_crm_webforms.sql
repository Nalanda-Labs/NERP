-- Web forms: public forms (hosted page or embedded on any website) that create leads, or
-- deals/smart-process records with their contact and company. Plus UTM tags on records.

ALTER TABLE crm_items ADD COLUMN utm JSONB;
CREATE INDEX crm_items_utm_source_idx ON crm_items ((utm->>'source') ASC) WHERE utm IS NOT NULL;

CREATE TABLE crm_webforms (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Public identifier used in links and embed code.
    key           TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL,
    -- What a submission creates: a lead, or a record of a type linked to a client.
    entity_type   SMALLINT NOT NULL REFERENCES crm_entity_types (id),
    category_id   UUID REFERENCES crm_categories (id) ON DELETE SET NULL,
    stage_id      UUID REFERENCES crm_stages (id) ON DELETE SET NULL,
    -- [{ key, label, kind, required, maps_to, options, placeholder }]
    fields        JSONB NOT NULL DEFAULT '[]',
    -- { title, description, button, success_message, redirect_url, color, consent_text }
    settings      JSONB NOT NULL DEFAULT '{}',
    -- Responsible people, assigned in turn.
    responsible   UUID[] NOT NULL DEFAULT '{}',
    -- Optional automatic reply from a connected mailbox.
    reply_mailbox UUID REFERENCES crm_mailboxes (id) ON DELETE SET NULL,
    reply_subject TEXT NOT NULL DEFAULT '',
    reply_body    TEXT NOT NULL DEFAULT '',
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    created_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE crm_webform_submissions (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    form_id     UUID NOT NULL REFERENCES crm_webforms (id) ON DELETE CASCADE,
    item_id     UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    contact_id  UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    -- [{ key, label, value }]
    data        JSONB NOT NULL,
    utm         JSONB,
    page_url    TEXT,
    referrer    TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_webform_submissions_form_idx ON crm_webform_submissions (form_id ASC, created_at DESC);
