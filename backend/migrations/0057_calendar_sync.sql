-- Two-way sync with outside calendars: a person connects a CalDAV account (iCloud, Nextcloud,
-- Fastmail, Yahoo…), Google Calendar or Microsoft 365. Events there come into NERP; their own
-- NERP events go there; changes and deletions go both ways.

CREATE TABLE cal_ext_accounts (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id           UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- caldav | google | microsoft
    kind              TEXT NOT NULL CHECK (kind IN ('caldav', 'google', 'microsoft')),
    -- The account (an email or user name) as shown.
    name              TEXT NOT NULL,
    -- CalDAV: the user name; Google / Microsoft: the account's email.
    username          TEXT,
    -- CalDAV: the password (encrypted); Google / Microsoft: the refresh token (encrypted).
    secret            BYTEA NOT NULL,
    access_token      BYTEA,
    access_expires_at TIMESTAMPTZ,
    -- The calendar synced: a CalDAV collection URL, or a Microsoft calendar id.
    calendar_url      TEXT NOT NULL,
    calendar_name     TEXT,
    -- Send NERP events there too (otherwise only bring theirs in).
    push              BOOLEAN NOT NULL DEFAULT TRUE,
    last_sync_at      TIMESTAMPTZ,
    last_error        TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, calendar_url)
);

-- Which NERP event is which item there, and the versions last seen on both sides.
CREATE TABLE cal_ext_links (
    account_id  UUID NOT NULL REFERENCES cal_ext_accounts (id) ON DELETE CASCADE,
    -- Not a foreign key: a link whose event is gone tells the next sync to delete it there.
    event_id    UUID NOT NULL,
    remote_id   TEXT NOT NULL,
    remote_etag TEXT,
    local_seq   BIGINT NOT NULL,
    PRIMARY KEY (account_id, event_id),
    UNIQUE (account_id, remote_id)
);

-- An event that came in from an outside calendar (it goes back only there).
ALTER TABLE cal_events ADD COLUMN ext_account_id UUID REFERENCES cal_ext_accounts (id) ON DELETE SET NULL;
CREATE INDEX cal_events_ext_idx ON cal_events (ext_account_id ASC) WHERE ext_account_id IS NOT NULL;
