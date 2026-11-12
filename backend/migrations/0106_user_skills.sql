-- What each employee is good at (languages, products, regions…), for assigning work to the
-- right person.
CREATE TABLE user_skills (
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    skill   TEXT NOT NULL,
    PRIMARY KEY (user_id, skill)
);
CREATE INDEX user_skills_skill_idx ON user_skills (lower(skill));
