-- HR documents to sign: employment contracts, policies, orders. A document is made from a
-- company template (filled in with each employee's details) or a file, for one employee or
-- many at once (one document each); it goes through steps in order: approvers approve, the
-- company's representative signs, the employee signs. When everyone has, a signed copy with a
-- certificate page is kept. HR can invalidate a signed document.

CREATE TABLE hr_doc_templates (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT NOT NULL,
    -- Text with {{fields}}: name, first_name, last_name, position, department, email, date, company.
    body       TEXT NOT NULL,
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE hr_docs (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title          TEXT NOT NULL,
    template_id    UUID REFERENCES hr_doc_templates (id) ON DELETE SET NULL,
    -- Sent together (to many employees at once).
    batch_id       UUID NOT NULL,
    employee_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- approval | signing | signed | declined | revoked
    status         TEXT NOT NULL CHECK (status IN ('approval', 'signing', 'signed', 'declined', 'revoked')),
    file_id        UUID REFERENCES files (id) ON DELETE SET NULL,
    sha256         TEXT NOT NULL,
    signed_file_id UUID REFERENCES files (id) ON DELETE SET NULL,
    signed_sha256  TEXT,
    due_on         DATE,
    created_by     UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at   TIMESTAMPTZ,
    revoked_at     TIMESTAMPTZ,
    revoked_by     UUID REFERENCES users (id) ON DELETE SET NULL,
    revoke_reason  TEXT
);
CREATE INDEX hr_docs_employee_idx ON hr_docs (employee_id ASC, created_at DESC);
CREATE INDEX hr_docs_batch_idx ON hr_docs (batch_id ASC);

CREATE TABLE hr_doc_steps (
    doc_id            UUID NOT NULL REFERENCES hr_docs (id) ON DELETE CASCADE,
    position          INT NOT NULL,
    user_id           UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- approver | company | employee
    role              TEXT NOT NULL CHECK (role IN ('approver', 'company', 'employee')),
    -- waiting (not their turn) | pending | done | declined
    status            TEXT NOT NULL DEFAULT 'waiting' CHECK (status IN ('waiting', 'pending', 'done', 'declined')),
    acted_at          TIMESTAMPTZ,
    comment           TEXT,
    signed_name       TEXT,
    signature_file_id UUID REFERENCES files (id) ON DELETE SET NULL,
    ip                TEXT,
    user_agent        TEXT,
    PRIMARY KEY (doc_id, position)
);
CREATE INDEX hr_doc_steps_user_idx ON hr_doc_steps (user_id ASC, status);
