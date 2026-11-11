-- Links to drive folders (and passwords on drive links); departments hidden from people
-- outside them.
ALTER TABLE drive_links ADD COLUMN password_hash TEXT;
ALTER TABLE departments ADD COLUMN hidden BOOLEAN NOT NULL DEFAULT FALSE;
