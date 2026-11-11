-- "Anyone with the link can edit": an office file opened from its link edits in the browser
-- (Collabora), each save a version made on behalf of the person who made the link.
ALTER TABLE drive_links ADD COLUMN can_edit BOOLEAN NOT NULL DEFAULT FALSE;
