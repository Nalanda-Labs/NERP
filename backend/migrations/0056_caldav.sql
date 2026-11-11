-- CalDAV: calendar apps (phones, Thunderbird, Apple Calendar, DAVx5…) read and change a
-- person's calendar two ways. They sign in with an app password, keep their own UID and
-- file name for events they make, may change one occurrence of a repeating event, and sync
-- only what changed since last time.

-- App passwords: for apps that can't sign in through the browser. Only the hash is kept.
CREATE TABLE app_passwords (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    last_used_ip TEXT
);
CREATE INDEX app_passwords_user_idx ON app_passwords (user_id ASC);

-- Every change to an event (or its guests, answers or removed occurrences) takes the next
-- number: the event's version for apps, and how a sync finds what changed.
CREATE SEQUENCE cal_dav_seq;
ALTER TABLE cal_events ADD COLUMN dav_seq BIGINT NOT NULL DEFAULT nextval('cal_dav_seq');
-- The iCalendar UID an app gave the event (NULL: "<id>@nerp").
ALTER TABLE cal_events ADD COLUMN uid TEXT;
-- The file name the app chose for it in the owner's calendar (NULL: "<id>.ics").
ALTER TABLE cal_events ADD COLUMN dav_name TEXT;
-- One occurrence of a repeating event changed on its own: it replaces the occurrence that
-- started at `recurrence_id` (which the parent lists as removed).
ALTER TABLE cal_events ADD COLUMN parent_id UUID REFERENCES cal_events (id) ON DELETE CASCADE;
ALTER TABLE cal_events ADD COLUMN recurrence_id TIMESTAMPTZ;
CREATE UNIQUE INDEX cal_events_uid_idx ON cal_events (owner_id, uid) WHERE uid IS NOT NULL AND parent_id IS NULL;
CREATE UNIQUE INDEX cal_events_dav_name_idx ON cal_events (owner_id, dav_name) WHERE dav_name IS NOT NULL;
CREATE INDEX cal_events_parent_idx ON cal_events (parent_id ASC) WHERE parent_id IS NOT NULL;
CREATE INDEX cal_events_dav_seq_idx ON cal_events (owner_id ASC, dav_seq ASC);

-- Events gone from someone's calendar (deleted, or they were taken off the guest list), so a
-- sync can tell their apps to remove them.
CREATE TABLE cal_dav_tombstones (
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    seq        BIGINT NOT NULL DEFAULT nextval('cal_dav_seq'),
    deleted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX cal_dav_tombstones_user_idx ON cal_dav_tombstones (user_id ASC, seq ASC);

-- A changed event takes a new number.
CREATE FUNCTION cal_dav_touch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW.dav_seq := nextval('cal_dav_seq');
    RETURN NEW;
END $$;
CREATE TRIGGER cal_events_dav_touch BEFORE UPDATE ON cal_events FOR EACH ROW EXECUTE FUNCTION cal_dav_touch();

-- A changed occurrence changes its repeating event too (apps get both in one file).
CREATE FUNCTION cal_dav_parent() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE p UUID;
BEGIN
    IF TG_OP = 'DELETE' THEN p := OLD.parent_id; ELSE p := NEW.parent_id; END IF;
    IF p IS NOT NULL THEN
        UPDATE cal_events SET dav_seq = nextval('cal_dav_seq') WHERE id = p;
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER cal_events_dav_parent AFTER INSERT OR UPDATE OR DELETE ON cal_events FOR EACH ROW EXECUTE FUNCTION cal_dav_parent();

-- Guests, their answers and removed occurrences are part of the event.
CREATE FUNCTION cal_dav_part() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE e UUID;
BEGIN
    IF TG_OP = 'DELETE' THEN e := OLD.event_id; ELSE e := NEW.event_id; END IF;
    UPDATE cal_events SET dav_seq = nextval('cal_dav_seq') WHERE id = e;
    RETURN NULL;
END $$;
CREATE TRIGGER cal_attendees_dav AFTER INSERT OR UPDATE OR DELETE ON cal_attendees FOR EACH ROW EXECUTE FUNCTION cal_dav_part();
CREATE TRIGGER cal_email_guests_dav AFTER INSERT OR UPDATE OR DELETE ON cal_email_guests FOR EACH ROW EXECUTE FUNCTION cal_dav_part();
CREATE TRIGGER cal_exceptions_dav AFTER INSERT OR UPDATE OR DELETE ON cal_exceptions FOR EACH ROW EXECUTE FUNCTION cal_dav_part();

-- A guest taken off the list loses the event from their calendar.
CREATE FUNCTION cal_dav_guest_gone() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO cal_dav_tombstones (user_id, name) VALUES (OLD.user_id, OLD.event_id::text || '.ics');
    RETURN NULL;
END $$;
CREATE TRIGGER cal_attendees_dav_gone AFTER DELETE ON cal_attendees FOR EACH ROW EXECUTE FUNCTION cal_dav_guest_gone();

-- A deleted event leaves the owner's calendar (under the app's name for it).
CREATE FUNCTION cal_dav_event_gone() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.parent_id IS NULL THEN
        INSERT INTO cal_dav_tombstones (user_id, name) VALUES (OLD.owner_id, COALESCE(OLD.dav_name, OLD.id::text || '.ics'));
    END IF;
    RETURN OLD;
END $$;
CREATE TRIGGER cal_events_dav_gone BEFORE DELETE ON cal_events FOR EACH ROW EXECUTE FUNCTION cal_dav_event_gone();
