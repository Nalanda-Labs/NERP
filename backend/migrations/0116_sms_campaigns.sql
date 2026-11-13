-- Marketing by text message: a campaign goes out by email or by SMS.
ALTER TABLE mkt_campaigns ADD COLUMN channel TEXT NOT NULL DEFAULT 'email' CHECK (channel IN ('email', 'sms'));
-- An SMS campaign's recipients have a phone number (and an empty email).
ALTER TABLE mkt_recipients ADD COLUMN phone TEXT;

-- Numbers that replied STOP (or were added by hand): no marketing texts to them.
CREATE TABLE sms_optouts (
    number     TEXT PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
