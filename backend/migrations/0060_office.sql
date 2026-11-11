-- Office documents in the browser: PDF previews of Word, Excel, PowerPoint and OpenDocument
-- files (made once with LibreOffice and kept), and editing through Collabora Online, where
-- every save is a new version of the drive file.

CREATE TABLE file_previews (
    file_id    UUID PRIMARY KEY REFERENCES files (id) ON DELETE CASCADE,
    pdf_id     UUID NOT NULL REFERENCES files (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
