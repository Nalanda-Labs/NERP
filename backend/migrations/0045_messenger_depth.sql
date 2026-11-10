-- Messenger depth: channels (posts with comments), open chats anyone can join, the company's
-- general chat and department chats whose members follow the company, roles in a chat,
-- polls, reactions, voice and video messages, stickers, chat folders, clearing history and
-- automatic deletion of old messages.

ALTER TABLE im_chats DROP CONSTRAINT im_chats_kind_check;
ALTER TABLE im_chats ADD CONSTRAINT im_chats_kind_check CHECK (kind IN ('direct', 'group', 'channel'));
ALTER TABLE im_chats ADD COLUMN description TEXT;
-- private: invited people only; open: listed for everyone, who can join.
ALTER TABLE im_chats ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private' CHECK (visibility IN ('private', 'open'));
-- Who the members are: chosen people (manual), every active employee (everyone), or a
-- department with its sub-departments (department).
ALTER TABLE im_chats ADD COLUMN audience TEXT NOT NULL DEFAULT 'manual' CHECK (audience IN ('manual', 'everyone', 'department'));
ALTER TABLE im_chats ADD COLUMN department_id UUID REFERENCES departments (id) ON DELETE CASCADE;
ALTER TABLE im_chats ADD CONSTRAINT im_chats_department_check CHECK ((audience = 'department') = (department_id IS NOT NULL));
-- The company's general chat (one).
ALTER TABLE im_chats ADD COLUMN is_general BOOLEAN NOT NULL DEFAULT FALSE;
CREATE UNIQUE INDEX im_chats_general_uidx ON im_chats (is_general) WHERE is_general;
-- all: every member writes; managers: only the chat's owner and admins (always for channels).
ALTER TABLE im_chats ADD COLUMN post_policy TEXT NOT NULL DEFAULT 'all' CHECK (post_policy IN ('all', 'managers'));
-- Messages older than this many hours are deleted.
ALTER TABLE im_chats ADD COLUMN auto_delete_hours INT CHECK (auto_delete_hours IS NULL OR auto_delete_hours > 0);
CREATE INDEX im_chats_open_idx ON im_chats (visibility) WHERE visibility = 'open';

-- owner | admin | member. The person who made a group chat is its owner.
ALTER TABLE im_members ADD COLUMN role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('owner', 'admin', 'member'));
-- Messages before this are hidden for this member ("Clear history for me").
ALTER TABLE im_members ADD COLUMN cleared_at TIMESTAMPTZ;
ALTER TABLE im_members ADD COLUMN muted BOOLEAN NOT NULL DEFAULT FALSE;
UPDATE im_members m SET role = 'owner' FROM im_chats c WHERE c.id = m.chat_id AND c.kind = 'group' AND c.created_by = m.user_id;

-- Comments under a channel post, or replies in a thread.
ALTER TABLE im_messages ADD COLUMN parent_id UUID REFERENCES im_messages (id) ON DELETE CASCADE;
-- text | poll | voice | video | sticker
ALTER TABLE im_messages ADD COLUMN kind TEXT NOT NULL DEFAULT 'text' CHECK (kind IN ('text', 'poll', 'voice', 'video', 'sticker'));
CREATE INDEX im_messages_parent_idx ON im_messages (parent_id ASC, created_at ASC) WHERE parent_id IS NOT NULL;

CREATE TABLE im_polls (
    message_id UUID PRIMARY KEY REFERENCES im_messages (id) ON DELETE CASCADE,
    question   TEXT NOT NULL,
    options    TEXT[] NOT NULL,
    multiple   BOOLEAN NOT NULL DEFAULT FALSE,
    anonymous  BOOLEAN NOT NULL DEFAULT FALSE,
    closed_at  TIMESTAMPTZ
);

CREATE TABLE im_poll_votes (
    message_id UUID NOT NULL REFERENCES im_polls (message_id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    option     INT NOT NULL,
    voted_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (message_id, user_id, option)
);

CREATE TABLE im_reactions (
    message_id UUID NOT NULL REFERENCES im_messages (id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    emoji      TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (message_id, user_id, emoji)
);

-- Each person's folders of chats.
CREATE TABLE im_folders (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    sort       INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX im_folders_user_idx ON im_folders (user_id ASC, sort ASC);

CREATE TABLE im_folder_chats (
    folder_id UUID NOT NULL REFERENCES im_folders (id) ON DELETE CASCADE,
    chat_id   UUID NOT NULL REFERENCES im_chats (id) ON DELETE CASCADE,
    PRIMARY KEY (folder_id, chat_id)
);
