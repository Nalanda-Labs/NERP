-- Attachments edited in the office editor: the file is updated in place (so every task,
-- message and post showing it shows the new content) and its earlier contents are kept.
ALTER TABLE files
    ADD COLUMN version    INT NOT NULL DEFAULT 1,
    ADD COLUMN updated_at TIMESTAMPTZ;

CREATE TABLE file_versions (
    file_id       UUID NOT NULL REFERENCES files (id) ON DELETE CASCADE,
    version       INT NOT NULL,
    storage_path  TEXT NOT NULL,
    enc_key       TEXT,
    size_bytes    BIGINT NOT NULL,
    sha256        TEXT NOT NULL,
    saved_by      UUID REFERENCES users (id) ON DELETE SET NULL,
    saved_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (file_id, version)
);
