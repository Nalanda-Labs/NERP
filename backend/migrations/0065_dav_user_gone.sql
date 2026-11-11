-- Removing a person (their events and invitations go with them) left CalDAV tombstones
-- pointing at nobody, which the foreign key refused, so the person could not be removed.
-- Nobody syncs a removed person's calendar: no tombstone then.

CREATE OR REPLACE FUNCTION cal_dav_guest_gone() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM users WHERE id = OLD.user_id) THEN
        INSERT INTO cal_dav_tombstones (user_id, name) VALUES (OLD.user_id, OLD.event_id::text || '.ics');
    END IF;
    RETURN NULL;
END $$;

CREATE OR REPLACE FUNCTION cal_dav_event_gone() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.parent_id IS NULL AND EXISTS (SELECT 1 FROM users WHERE id = OLD.owner_id) THEN
        INSERT INTO cal_dav_tombstones (user_id, name) VALUES (OLD.owner_id, COALESCE(OLD.dav_name, OLD.id::text || '.ics'));
    END IF;
    RETURN OLD;
END $$;
