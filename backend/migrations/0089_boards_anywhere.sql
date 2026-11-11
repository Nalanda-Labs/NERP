-- Boards made from a feed post (its readers open it, to look or to draw) or in a drive folder
-- (where it shows as an item; the folder's people open it as the drive lets them).
ALTER TABLE boards ADD COLUMN post_id UUID REFERENCES feed_posts (id) ON DELETE SET NULL;
ALTER TABLE boards ADD COLUMN post_level TEXT NOT NULL DEFAULT 'edit' CHECK (post_level IN ('view', 'edit'));
ALTER TABLE drive_nodes ADD COLUMN board_id UUID REFERENCES boards (id) ON DELETE CASCADE;
ALTER TABLE boards ADD COLUMN drive_node_id UUID REFERENCES drive_nodes (id) ON DELETE SET NULL;
CREATE INDEX boards_post_idx ON boards (post_id) WHERE post_id IS NOT NULL;
