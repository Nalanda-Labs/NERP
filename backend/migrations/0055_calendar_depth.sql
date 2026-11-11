-- Calendar depth: guests from outside the company invited by email (iCalendar invitations with
-- an answer link), meeting rooms (the booking module's rooms, so a room is never double-booked
-- by an event and a booking), who may see or change a person's calendar, and links where
-- others pick a free time (one person's open slots, or slots free for several people).

-- Changes that matter to guests (time, place, title) raise the sequence, so their calendar
-- apps replace the old copy of the invitation.
ALTER TABLE cal_events ADD COLUMN sequence INT NOT NULL DEFAULT 0;
-- Who put it there (the owner, or a colleague allowed to change the owner's calendar).
ALTER TABLE cal_events ADD COLUMN created_by UUID REFERENCES users (id) ON DELETE SET NULL;
-- A meeting room (a booking resource of kind 'room').
ALTER TABLE cal_events ADD COLUMN room_id UUID REFERENCES booking_resources (id) ON DELETE SET NULL;
CREATE INDEX cal_events_room_idx ON cal_events (room_id ASC, starts_at ASC) WHERE room_id IS NOT NULL;
-- Booked through a slot link.
ALTER TABLE cal_events ADD COLUMN slot_link_id UUID;

-- Guests outside the company, by email. Their answer link is signed (no token is stored).
CREATE TABLE cal_email_guests (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id   UUID NOT NULL REFERENCES cal_events (id) ON DELETE CASCADE,
    email      TEXT NOT NULL,
    name       TEXT,
    status     TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'declined', 'tentative')),
    answered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (event_id, email)
);

-- Who may see or change a person's calendar. Without rows everyone sees it in full (private
-- events as "Busy"). A row for one person wins over their departments', which win over the
-- row for everyone.
--   none: can't open it · busy: times only · read: details · edit: details, private events
--   too, and may add, change and remove events on it.
CREATE TABLE cal_access (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    grantee_kind TEXT NOT NULL CHECK (grantee_kind IN ('everyone', 'department', 'user')),
    grantee_id   UUID,
    level        TEXT NOT NULL CHECK (level IN ('none', 'busy', 'read', 'edit')),
    CHECK ((grantee_kind = 'everyone') = (grantee_id IS NULL))
);
CREATE UNIQUE INDEX cal_access_one_idx ON cal_access (owner_id, grantee_kind, COALESCE(grantee_id, '00000000-0000-0000-0000-000000000000'::uuid));
CREATE INDEX cal_access_grantee_idx ON cal_access (grantee_id ASC) WHERE grantee_id IS NOT NULL;

-- Slot links at /meet/<slug>: someone outside (or inside) picks a free time and an event is
-- made with the owner and the other members. With members it offers only times when all of
-- them are free (shared slots).
CREATE TABLE cal_slot_links (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id           UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    slug               TEXT NOT NULL UNIQUE,
    title              TEXT NOT NULL,
    description        TEXT,
    location           TEXT,
    duration_minutes   INT NOT NULL CHECK (duration_minutes BETWEEN 5 AND 480),
    -- Others who must be free too, and who are invited.
    members            UUID[] NOT NULL DEFAULT '{}',
    -- [[weekday 1-7, from minute, to minute], …] in the link's time zone.
    hours              JSONB NOT NULL,
    timezone           TEXT NOT NULL,
    slot_step_minutes  INT NOT NULL DEFAULT 30 CHECK (slot_step_minutes BETWEEN 5 AND 240),
    buffer_minutes     INT NOT NULL DEFAULT 0 CHECK (buffer_minutes BETWEEN 0 AND 240),
    min_notice_minutes INT NOT NULL DEFAULT 120 CHECK (min_notice_minutes BETWEEN 0 AND 20160),
    days_ahead         INT NOT NULL DEFAULT 30 CHECK (days_ahead BETWEEN 1 AND 365),
    -- The most meetings a day through this link (NULL: no limit).
    max_per_day        INT CHECK (max_per_day IS NULL OR max_per_day BETWEEN 1 AND 50),
    remind_minutes     INT,
    is_active          BOOLEAN NOT NULL DEFAULT TRUE,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX cal_slot_links_owner_idx ON cal_slot_links (owner_id ASC);
ALTER TABLE cal_events ADD CONSTRAINT cal_events_slot_link_fk FOREIGN KEY (slot_link_id) REFERENCES cal_slot_links (id) ON DELETE SET NULL;
