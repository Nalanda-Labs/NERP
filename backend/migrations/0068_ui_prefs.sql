-- How each person arranges the app: sidebar items hidden or pinned to the top, and the
-- background. The company's default (and "apply to everyone") lives in `settings` (ui_default).
CREATE TABLE user_ui_prefs (
    user_id    UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- {"hidden": ["/boards"], "pinned": ["/crm/deal"]}
    menu       JSONB,
    -- A preset name or an uploaded (public) image's file id.
    background TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
