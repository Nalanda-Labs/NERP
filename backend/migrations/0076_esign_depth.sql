-- E-signature depth: signing templates (who signs, the message, and optionally the document
-- template to make the document from), documents waiting to be sent once they are ready,
-- and what happens after an HR document is signed.

CREATE TABLE sign_templates (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL,
    message         TEXT,
    -- Make the document from this template first (else: the record's newest ready document).
    doc_template_id UUID REFERENCES crm_doc_templates (id) ON DELETE SET NULL,
    -- [{ "role": "contact" | "company" | "record" | "responsible" | "sender" | "person",
    --    "name": "...", "email": "..." }]   (name and email for "person" only)
    signers         JSONB NOT NULL DEFAULT '[]',
    -- Record types it is offered for (empty: all).
    entity_types    SMALLINT[] NOT NULL DEFAULT '{}',
    created_by      UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE sign_requests ADD COLUMN template_id UUID REFERENCES sign_templates (id) ON DELETE SET NULL;

-- A document being made (in the job queue) that goes out for signature when it is ready.
CREATE TABLE sign_pending (
    document_id  UUID PRIMARY KEY REFERENCES crm_documents (id) ON DELETE CASCADE,
    template_id  UUID NOT NULL REFERENCES sign_templates (id) ON DELETE CASCADE,
    requested_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- After an HR document is signed by everyone: { "notify": [user ids], "task": { "title",
-- "responsible": "hr" | "employee" | user id }, "list_id": a list to add an item to (which
-- starts that list's processes) }.
ALTER TABLE hr_doc_templates ADD COLUMN after_signing JSONB NOT NULL DEFAULT '{}';
