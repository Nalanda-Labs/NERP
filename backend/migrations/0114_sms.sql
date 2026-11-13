-- SMS through Twilio: every message sent or received, with its delivery status.
CREATE TABLE sms_messages (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    direction    TEXT NOT NULL CHECK (direction IN ('out', 'in')),
    to_number    TEXT NOT NULL,
    from_number  TEXT,
    body         TEXT NOT NULL,
    -- queued (waiting for the worker) | sending (handed to Twilio) | sent | delivered |
    -- undelivered | failed | received
    status       TEXT NOT NULL DEFAULT 'queued',
    error        TEXT,
    -- Twilio's message SID (SM…), set once Twilio accepts the message.
    provider_sid TEXT UNIQUE,
    segments     INT,
    -- crm | robot | booking | esign | campaign | invite | test | reply
    purpose      TEXT NOT NULL,
    item_id      UUID REFERENCES crm_items (id) ON DELETE SET NULL,
    campaign_id  UUID,
    booking_id   UUID REFERENCES bookings (id) ON DELETE SET NULL,
    -- Who sent it (NULL: the system).
    user_id      UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX sms_messages_item ON sms_messages (item_id, created_at DESC) WHERE item_id IS NOT NULL;
CREATE INDEX sms_messages_created ON sms_messages (created_at DESC);
CREATE INDEX sms_messages_campaign ON sms_messages (campaign_id) WHERE campaign_id IS NOT NULL;

-- Text messages are CRM activities like calls and emails.
ALTER TABLE crm_activities DROP CONSTRAINT crm_activities_kind_check;
ALTER TABLE crm_activities ADD CONSTRAINT crm_activities_kind_check CHECK (kind IN ('todo', 'call', 'meeting', 'email', 'visit', 'sms'));
ALTER TABLE crm_activities ADD COLUMN sms_id UUID REFERENCES sms_messages (id) ON DELETE SET NULL;

-- Booking messages to the client by text as well as email.
ALTER TABLE booking_settings ADD COLUMN sms_notices BOOLEAN NOT NULL DEFAULT FALSE;
