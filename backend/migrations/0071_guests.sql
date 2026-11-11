-- Guests: people from outside the company invited into chats and workgroups. They see only
-- those chats and workgroups (and the tasks, files and calls in them), never the directory,
-- the company feed or anything shared with "everyone". They have the "guest" role, without
-- permissions.
ALTER TABLE users ADD COLUMN is_guest BOOLEAN NOT NULL DEFAULT FALSE;
INSERT INTO roles (code, name, description, is_system)
VALUES ('guest', 'Guest', 'People from outside the company, in the chats and workgroups they are invited to', TRUE)
ON CONFLICT (code) DO NOTHING;
