-- Workgroups: projects become group spaces, each with its own news feed, files, chat and
-- calendar besides its tasks. A group is either a workgroup (ongoing) or a project (with
-- start and end dates).

ALTER TABLE projects ADD COLUMN kind TEXT NOT NULL DEFAULT 'project' CHECK (kind IN ('group', 'project'));
ALTER TABLE projects ADD COLUMN starts_on DATE;
ALTER TABLE projects ADD COLUMN ends_on DATE;
-- open: everyone sees it and can join; closed: listed, members-only content, joining by
-- request; private: only members know it exists.
ALTER TABLE projects DROP CONSTRAINT projects_visibility_check;
ALTER TABLE projects ADD CONSTRAINT projects_visibility_check CHECK (visibility IN ('open', 'closed', 'private'));

CREATE TABLE project_join_requests (
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    message    TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, user_id)
);

-- Posts in a group's feed (seen by its members, and by everyone for open groups).
ALTER TABLE feed_posts ADD COLUMN project_id UUID REFERENCES projects (id) ON DELETE CASCADE;
CREATE INDEX feed_posts_project_idx ON feed_posts (project_id ASC, created_at DESC) WHERE project_id IS NOT NULL;

-- The group's files: a drive space of its own.
ALTER TABLE drive_nodes ADD COLUMN project_id UUID REFERENCES projects (id) ON DELETE CASCADE;
ALTER TABLE drive_nodes DROP CONSTRAINT drive_nodes_space_check;
ALTER TABLE drive_nodes ADD CONSTRAINT drive_nodes_space_check CHECK (space IN ('personal', 'company', 'group'));
ALTER TABLE drive_nodes ADD CONSTRAINT drive_nodes_group_check CHECK ((space = 'group') = (project_id IS NOT NULL));
CREATE INDEX drive_nodes_project_idx ON drive_nodes (project_id ASC, parent_id ASC) WHERE project_id IS NOT NULL;

-- The group's chat: its members are the group's members.
ALTER TABLE im_chats ADD COLUMN project_id UUID UNIQUE REFERENCES projects (id) ON DELETE CASCADE;

-- Events on the group's calendar.
ALTER TABLE cal_events ADD COLUMN project_id UUID REFERENCES projects (id) ON DELETE CASCADE;
CREATE INDEX cal_events_project_idx ON cal_events (project_id ASC, starts_at ASC) WHERE project_id IS NOT NULL;
