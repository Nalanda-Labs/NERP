-- Proactive CRM: each person is told daily which of their deals and leads need attention.
ALTER TABLE crm_insight_settings
    ADD COLUMN attention_digest BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN attention_email  BOOLEAN NOT NULL DEFAULT FALSE,
    -- Hours a client's email may wait for an answer before it counts.
    ADD COLUMN reply_hours      INT NOT NULL DEFAULT 24 CHECK (reply_hours BETWEEN 1 AND 720);
