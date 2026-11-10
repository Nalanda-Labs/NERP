-- Training: courses of chapters and lessons, questions, tests with attempts, a gradebook,
-- assignments with due dates, and certificates.

CREATE TABLE training_courses (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title         TEXT NOT NULL,
    -- Markdown.
    description   TEXT NOT NULL DEFAULT '',
    cover_file_id UUID REFERENCES files (id) ON DELETE SET NULL,
    -- Drafts are seen only by people who manage training.
    is_published  BOOLEAN NOT NULL DEFAULT FALSE,
    -- Everyone may enrol, or only the people and departments in training_access.
    to_all        BOOLEAN NOT NULL DEFAULT TRUE,
    sort          INT NOT NULL DEFAULT 500,
    created_by    UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE training_access (
    course_id     UUID NOT NULL REFERENCES training_courses (id) ON DELETE CASCADE,
    user_id       UUID REFERENCES users (id) ON DELETE CASCADE,
    -- A department includes its sub-departments.
    department_id UUID REFERENCES departments (id) ON DELETE CASCADE,
    CHECK ((user_id IS NULL) <> (department_id IS NULL))
);
CREATE INDEX training_access_course_idx ON training_access (course_id ASC);

CREATE TABLE training_chapters (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    course_id  UUID NOT NULL REFERENCES training_courses (id) ON DELETE CASCADE,
    title      TEXT NOT NULL,
    sort       INT NOT NULL DEFAULT 500
);
CREATE INDEX training_chapters_course_idx ON training_chapters (course_id ASC);

CREATE TABLE training_lessons (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    course_id        UUID NOT NULL REFERENCES training_courses (id) ON DELETE CASCADE,
    chapter_id       UUID REFERENCES training_chapters (id) ON DELETE SET NULL,
    title            TEXT NOT NULL,
    -- Markdown.
    body             TEXT NOT NULL DEFAULT '',
    video_url        TEXT,
    file_ids         UUID[] NOT NULL DEFAULT '{}',
    -- Opens this many days after the person enrolled (0: at once).
    opens_after_days INT NOT NULL DEFAULT 0 CHECK (opens_after_days BETWEEN 0 AND 3650),
    sort             INT NOT NULL DEFAULT 500,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX training_lessons_course_idx ON training_lessons (course_id ASC);

CREATE TABLE training_questions (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    course_id        UUID NOT NULL REFERENCES training_courses (id) ON DELETE CASCADE,
    -- The lesson it is about (tests can draw from one lesson).
    lesson_id        UUID REFERENCES training_lessons (id) ON DELETE SET NULL,
    -- single | multiple | order (put the answers in the right order) | text (written, graded by hand)
    kind             TEXT NOT NULL CHECK (kind IN ('single', 'multiple', 'order', 'text')),
    -- Markdown.
    text             TEXT NOT NULL,
    points           INT NOT NULL DEFAULT 10 CHECK (points BETWEEN 0 AND 1000),
    -- The test isn't passed without answering this one correctly, whatever the score.
    required_correct BOOLEAN NOT NULL DEFAULT FALSE,
    -- Shown when it is answered wrongly.
    explanation      TEXT,
    -- A quick check inside its lesson, not used in tests.
    self_check       BOOLEAN NOT NULL DEFAULT FALSE,
    sort             INT NOT NULL DEFAULT 500,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX training_questions_course_idx ON training_questions (course_id ASC);

-- Choices of single and multiple questions; for order questions, the items in their right order.
CREATE TABLE training_answers (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    question_id UUID NOT NULL REFERENCES training_questions (id) ON DELETE CASCADE,
    text        TEXT NOT NULL,
    is_correct  BOOLEAN NOT NULL DEFAULT FALSE,
    sort        INT NOT NULL DEFAULT 0
);
CREATE INDEX training_answers_question_idx ON training_answers (question_id ASC);

CREATE TABLE training_tests (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    course_id             UUID NOT NULL REFERENCES training_courses (id) ON DELETE CASCADE,
    title                 TEXT NOT NULL,
    description           TEXT NOT NULL DEFAULT '',
    is_active             BOOLEAN NOT NULL DEFAULT TRUE,
    -- Questions from one lesson, or (NULL) the whole course; self-check questions never.
    lesson_id             UUID REFERENCES training_lessons (id) ON DELETE SET NULL,
    -- How many, picked at random (0: all).
    question_count        INT NOT NULL DEFAULT 0 CHECK (question_count >= 0),
    shuffle_questions     BOOLEAN NOT NULL DEFAULT TRUE,
    shuffle_answers       BOOLEAN NOT NULL DEFAULT TRUE,
    -- strict: answer each in turn, no going back; skip: may skip, answers final; free: go anywhere, change answers.
    navigation            TEXT NOT NULL DEFAULT 'free' CHECK (navigation IN ('strict', 'skip', 'free')),
    -- When the right answers are shown: never | after_each | at_end
    show_answers          TEXT NOT NULL DEFAULT 'at_end' CHECK (show_answers IN ('never', 'after_each', 'at_end')),
    attempt_limit         INT NOT NULL DEFAULT 0 CHECK (attempt_limit >= 0),
    time_limit_minutes    INT NOT NULL DEFAULT 0 CHECK (time_limit_minutes BETWEEN 0 AND 1440),
    -- Minutes to wait before trying again.
    wait_minutes          INT NOT NULL DEFAULT 0 CHECK (wait_minutes >= 0),
    pass_percent          INT NOT NULL DEFAULT 70 CHECK (pass_percent BETWEEN 0 AND 100),
    -- Taken only after passing this test with at least this score.
    prerequisite_test_id  UUID REFERENCES training_tests (id) ON DELETE SET NULL,
    prerequisite_percent  INT NOT NULL DEFAULT 0 CHECK (prerequisite_percent BETWEEN 0 AND 100),
    sort                  INT NOT NULL DEFAULT 500,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX training_tests_course_idx ON training_tests (course_id ASC);

-- Marks by score: "Excellent" from 90 %, "Good" from 75 %…
CREATE TABLE training_test_marks (
    test_id     UUID NOT NULL REFERENCES training_tests (id) ON DELETE CASCADE,
    min_percent INT NOT NULL CHECK (min_percent BETWEEN 0 AND 100),
    mark        TEXT NOT NULL,
    PRIMARY KEY (test_id, min_percent)
);

CREATE TABLE training_enrolments (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    course_id    UUID NOT NULL REFERENCES training_courses (id) ON DELETE CASCADE,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- self | assigned
    source       TEXT NOT NULL DEFAULT 'self' CHECK (source IN ('self', 'assigned')),
    assigned_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    due_on       DATE,
    reminded_at  TIMESTAMPTZ,
    enrolled_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    UNIQUE (course_id, user_id)
);
CREATE INDEX training_enrolments_user_idx ON training_enrolments (user_id ASC);

CREATE TABLE training_lesson_progress (
    lesson_id    UUID NOT NULL REFERENCES training_lessons (id) ON DELETE CASCADE,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    completed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (lesson_id, user_id)
);

CREATE TABLE training_attempts (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    test_id      UUID NOT NULL REFERENCES training_tests (id) ON DELETE CASCADE,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    started_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- The time limit runs out then.
    deadline_at  TIMESTAMPTZ,
    finished_at  TIMESTAMPTZ,
    -- in_progress | review (written answers wait for grading) | done
    status       TEXT NOT NULL DEFAULT 'in_progress' CHECK (status IN ('in_progress', 'review', 'done')),
    score        INT NOT NULL DEFAULT 0,
    max_score    INT NOT NULL DEFAULT 0,
    percent      INT NOT NULL DEFAULT 0,
    passed       BOOLEAN,
    mark         TEXT
);
CREATE INDEX training_attempts_user_idx ON training_attempts (user_id ASC, test_id ASC);

-- The questions of an attempt, in the order shown, and what was answered.
CREATE TABLE training_attempt_answers (
    attempt_id   UUID NOT NULL REFERENCES training_attempts (id) ON DELETE CASCADE,
    question_id  UUID NOT NULL REFERENCES training_questions (id) ON DELETE CASCADE,
    position     INT NOT NULL,
    -- The choices in the order shown to this person.
    answer_order UUID[] NOT NULL DEFAULT '{}',
    answered     BOOLEAN NOT NULL DEFAULT FALSE,
    -- Chosen answer ids (single, multiple, order) or the written text.
    chosen       UUID[] NOT NULL DEFAULT '{}',
    written      TEXT,
    -- NULL until graded (written answers wait for a person).
    correct      BOOLEAN,
    points       INT NOT NULL DEFAULT 0,
    reviewed_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    review_note  TEXT,
    PRIMARY KEY (attempt_id, question_id)
);

-- Attempts granted beyond a test's limit, per person.
CREATE TABLE training_extra_attempts (
    test_id  UUID NOT NULL REFERENCES training_tests (id) ON DELETE CASCADE,
    user_id  UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    extra    INT NOT NULL DEFAULT 0 CHECK (extra >= 0),
    PRIMARY KEY (test_id, user_id)
);

CREATE SEQUENCE training_certificate_seq;
CREATE TABLE training_certificates (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Consecutive, for the printed certificate.
    number     BIGINT NOT NULL DEFAULT nextval('training_certificate_seq') UNIQUE,
    course_id  UUID NOT NULL REFERENCES training_courses (id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    issued_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Best results over the course's tests.
    score      INT NOT NULL DEFAULT 0,
    max_score  INT NOT NULL DEFAULT 0,
    UNIQUE (course_id, user_id)
);
