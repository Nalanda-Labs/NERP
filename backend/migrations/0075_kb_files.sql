-- Pictures and files placed in knowledge base pages: whoever may read the page may open them.
CREATE TABLE kb_files (
    page_id    UUID NOT NULL REFERENCES kb_pages (id) ON DELETE CASCADE,
    file_id    UUID NOT NULL REFERENCES files (id) ON DELETE CASCADE,
    added_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (page_id, file_id)
);
CREATE INDEX kb_files_file_idx ON kb_files (file_id);
