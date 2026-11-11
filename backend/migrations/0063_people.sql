-- People: reactions on feed posts (not only likes), appreciation badges given to colleagues
-- in the feed (and counted on their profiles), and when someone was last active.

ALTER TABLE feed_likes ADD COLUMN reaction TEXT NOT NULL DEFAULT 'like' CHECK (reaction IN ('like', 'love', 'laugh', 'wow', 'sad', 'clap'));

-- An appreciation post: a badge given to colleagues.
ALTER TABLE feed_posts ADD COLUMN badge TEXT CHECK (badge IS NULL OR badge IN ('thanks', 'team_player', 'great_idea', 'problem_solver', 'leader', 'above_beyond'));
CREATE TABLE feed_badges (
    post_id UUID NOT NULL REFERENCES feed_posts (id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (post_id, user_id)
);
CREATE INDEX feed_badges_user_idx ON feed_badges (user_id ASC);

ALTER TABLE users ADD COLUMN last_seen_at TIMESTAMPTZ;
