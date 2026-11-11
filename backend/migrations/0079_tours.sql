-- Guided tours people have finished (so they are offered once).
ALTER TABLE users ADD COLUMN tours_done TEXT[] NOT NULL DEFAULT '{}';
