-- CRM → Documents lists every document, newest first.
CREATE INDEX crm_documents_created_idx ON crm_documents (created_at DESC, id DESC);
