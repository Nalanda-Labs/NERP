-- CRM documents (phase 2): templates in Typst, LuaLaTeX or LibreOffice (DOCX) format and
-- the PDFs generated from them.

CREATE TABLE crm_doc_templates (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name           TEXT NOT NULL,
    engine         TEXT NOT NULL CHECK (engine IN ('typst', 'latex', 'office')),
    -- Record types this template is offered for (crm_entity_types.id).
    entity_types   SMALLINT[] NOT NULL DEFAULT '{2}',
    -- Printed title, e.g. "Invoice", "Quote", "Rechnung".
    doc_title      TEXT NOT NULL DEFAULT 'Invoice',
    -- BCP 47 tag for number/date formatting; empty = the company default.
    locale         TEXT NOT NULL DEFAULT '',
    -- Typst / LaTeX source. Office templates keep their DOCX in files.
    source         TEXT,
    file_id        UUID REFERENCES files (id) ON DELETE SET NULL,
    -- Document numbers: prefix (with {YYYY}, {YY}, {MM}) plus a gap-free counter.
    number_prefix  TEXT NOT NULL DEFAULT 'INV-{YYYY}-',
    builtin_key    TEXT UNIQUE,
    is_active      BOOLEAN NOT NULL DEFAULT TRUE,
    created_by     UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_doc_templates_name_idx ON crm_doc_templates (name ASC);

-- One counter per resolved prefix ("INV-2026-"), so templates sharing a prefix share a
-- sequence and every year starts again at 1.
CREATE TABLE crm_doc_counters (
    prefix  TEXT PRIMARY KEY,
    value   BIGINT NOT NULL
);

CREATE TABLE crm_documents (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    item_id         UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    template_id     UUID REFERENCES crm_doc_templates (id) ON DELETE SET NULL,
    template_name   TEXT NOT NULL,
    engine          TEXT NOT NULL,
    number          TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'ready', 'failed')),
    pdf_file_id     UUID REFERENCES files (id) ON DELETE SET NULL,
    -- The filled DOCX for LibreOffice templates.
    source_file_id  UUID REFERENCES files (id) ON DELETE SET NULL,
    error           TEXT,
    created_by      UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_documents_item_idx ON crm_documents (item_id ASC, created_at DESC);
CREATE INDEX crm_documents_pdf_idx ON crm_documents (pdf_file_id ASC) WHERE pdf_file_id IS NOT NULL;
CREATE INDEX crm_documents_source_idx ON crm_documents (source_file_id ASC) WHERE source_file_id IS NOT NULL;
