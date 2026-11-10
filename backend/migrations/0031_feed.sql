-- News feed: posts to everyone or to chosen people and departments (with their
-- sub-departments), with files, likes, comments, polls, and important posts whose readers
-- are tracked.

CREATE TABLE feed_posts (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    author_id  UUID REFERENCES users (id) ON DELETE SET NULL,
    body       TEXT NOT NULL,
    file_ids   UUID[] NOT NULL DEFAULT '{}',
    to_all     BOOLEAN NOT NULL DEFAULT TRUE,
    important  BOOLEAN NOT NULL DEFAULT FALSE,
    -- {"question": "...", "options": ["...", "..."]}
    poll       JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    edited_at  TIMESTAMPTZ
);
CREATE INDEX feed_posts_created_idx ON feed_posts (created_at DESC);

CREATE TABLE feed_audience (
    post_id       UUID NOT NULL REFERENCES feed_posts (id) ON DELETE CASCADE,
    user_id       UUID REFERENCES users (id) ON DELETE CASCADE,
    department_id UUID REFERENCES departments (id) ON DELETE CASCADE,
    CHECK ((user_id IS NULL) <> (department_id IS NULL))
);
CREATE INDEX feed_audience_post_idx ON feed_audience (post_id ASC);

CREATE TABLE feed_comments (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    post_id    UUID NOT NULL REFERENCES feed_posts (id) ON DELETE CASCADE,
    author_id  UUID REFERENCES users (id) ON DELETE SET NULL,
    body       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX feed_comments_post_idx ON feed_comments (post_id ASC, created_at ASC);

CREATE TABLE feed_likes (
    post_id UUID NOT NULL REFERENCES feed_posts (id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (post_id, user_id)
);

-- Important posts: who has confirmed reading them.
CREATE TABLE feed_reads (
    post_id UUID NOT NULL REFERENCES feed_posts (id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    read_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (post_id, user_id)
);

CREATE TABLE feed_votes (
    post_id UUID NOT NULL REFERENCES feed_posts (id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    option  INT NOT NULL,
    PRIMARY KEY (post_id, user_id)
);
