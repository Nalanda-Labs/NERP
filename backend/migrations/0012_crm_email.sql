-- CRM email: employees connect their mailboxes (IMAP + SMTP). Messages to and from CRM
-- clients are filed on the matching leads, contacts, companies and deals; replies can be
-- sent from the record. Only CRM-related messages are stored.

CREATE TABLE crm_mailboxes (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id             UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    email                TEXT NOT NULL,
    display_name         TEXT NOT NULL DEFAULT '',
    imap_host            TEXT NOT NULL,
    imap_port            INT NOT NULL,
    imap_security        TEXT NOT NULL CHECK (imap_security IN ('tls', 'starttls', 'none')),
    smtp_host            TEXT NOT NULL,
    smtp_port            INT NOT NULL,
    smtp_security        TEXT NOT NULL CHECK (smtp_security IN ('tls', 'starttls', 'none')),
    smtp_auth            BOOLEAN NOT NULL DEFAULT TRUE,
    username             TEXT NOT NULL,
    -- The password, encrypted with AES-256-GCM (see crypto.rs).
    secret               BYTEA NOT NULL,
    -- Self-signed certificates (local or test servers only).
    allow_invalid_certs  BOOLEAN NOT NULL DEFAULT FALSE,
    -- Colleagues who may also send from this mailbox.
    shared_with          UUID[] NOT NULL DEFAULT '{}',
    -- Create a lead for messages from unknown senders.
    create_leads         BOOLEAN NOT NULL DEFAULT FALSE,
    -- How far back the first sync looks.
    sync_days            INT NOT NULL DEFAULT 7 CHECK (sync_days BETWEEN 0 AND 365),
    sync_enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    -- Per folder: { "INBOX": { "validity": 123, "last_uid": 456 }, … }
    folders              JSONB NOT NULL DEFAULT '{}',
    sent_folder          TEXT,
    last_sync_at         TIMESTAMPTZ,
    last_error           TEXT,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crm_mailboxes_owner_idx ON crm_mailboxes (owner_id ASC);

CREATE TABLE crm_emails (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    mailbox_id    UUID REFERENCES crm_mailboxes (id) ON DELETE SET NULL,
    direction     TEXT NOT NULL CHECK (direction IN ('in', 'out')),
    -- received | queued | sent | failed
    status        TEXT NOT NULL DEFAULT 'received',
    message_id    TEXT NOT NULL,
    in_reply_to   TEXT,
    refs          TEXT[] NOT NULL DEFAULT '{}',
    -- Messages of one conversation share the id of its first message.
    thread_id     UUID NOT NULL,
    from_addr     TEXT NOT NULL DEFAULT '',
    from_name     TEXT NOT NULL DEFAULT '',
    to_addrs      JSONB NOT NULL DEFAULT '[]',
    cc_addrs      JSONB NOT NULL DEFAULT '[]',
    subject       TEXT NOT NULL DEFAULT '',
    body_text     TEXT,
    -- Sanitised HTML (scripts, styles that load things, event handlers removed).
    body_html     TEXT,
    snippet       TEXT NOT NULL DEFAULT '',
    attachments   JSONB NOT NULL DEFAULT '[]',
    sent_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    error         TEXT,
    created_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX crm_emails_mailbox_msg_uidx ON crm_emails (mailbox_id, message_id);
CREATE INDEX crm_emails_message_idx ON crm_emails (message_id ASC);
CREATE INDEX crm_emails_thread_idx ON crm_emails (thread_id ASC, sent_at ASC);

CREATE TABLE crm_email_links (
    email_id  UUID NOT NULL REFERENCES crm_emails (id) ON DELETE CASCADE,
    item_id   UUID NOT NULL REFERENCES crm_items (id) ON DELETE CASCADE,
    PRIMARY KEY (email_id, item_id)
);
CREATE INDEX crm_email_links_item_idx ON crm_email_links (item_id ASC);

ALTER TABLE crm_activities ADD COLUMN email_id UUID REFERENCES crm_emails (id) ON DELETE SET NULL;

-- Attachments stored as files; readable by whoever can read a record the email is on.
CREATE TABLE crm_email_files (
    file_id   UUID PRIMARY KEY REFERENCES files (id) ON DELETE CASCADE,
    email_id  UUID NOT NULL REFERENCES crm_emails (id) ON DELETE CASCADE
);
CREATE INDEX crm_email_files_email_idx ON crm_email_files (email_id ASC);
