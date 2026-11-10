-- NERP core schema: users, org structure, roles/permissions, files, settings, audit log.
-- Written to be compatible with YugabyteDB YSQL (PostgreSQL 11+ dialect).
-- UUID primary keys are used deliberately: they spread writes across tablets
-- instead of hot-spotting a single range the way sequential ids would.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE files (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id       UUID,
    original_name  TEXT NOT NULL,
    mime_type      TEXT NOT NULL,
    size_bytes     BIGINT NOT NULL,
    sha256         TEXT NOT NULL,
    storage_path   TEXT NOT NULL,
    is_public      BOOLEAN NOT NULL DEFAULT FALSE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX files_owner_idx ON files (owner_id, created_at DESC);

CREATE TABLE users (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email           TEXT NOT NULL,
    password_hash   TEXT NOT NULL,
    first_name      TEXT NOT NULL,
    last_name       TEXT NOT NULL,
    middle_name     TEXT,
    position        TEXT,
    phone           TEXT,
    mobile          TEXT,
    birthday        DATE,
    gender          TEXT CHECK (gender IN ('male', 'female', 'other')),
    about           TEXT,
    avatar_file_id  UUID REFERENCES files (id) ON DELETE SET NULL,
    timezone        TEXT NOT NULL DEFAULT 'UTC',
    locale          TEXT NOT NULL DEFAULT 'en',
    is_active       BOOLEAN NOT NULL DEFAULT TRUE,
    last_login_at   TIMESTAMPTZ,
    password_changed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Emails are normalised to lower case by the application.
CREATE UNIQUE INDEX users_email_uidx ON users (email);
CREATE INDEX users_name_idx ON users (last_name, first_name);

ALTER TABLE files
    ADD CONSTRAINT files_owner_fk FOREIGN KEY (owner_id) REFERENCES users (id) ON DELETE SET NULL;

CREATE TABLE departments (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    parent_id     UUID REFERENCES departments (id) ON DELETE RESTRICT,
    name          TEXT NOT NULL,
    description   TEXT,
    head_user_id  UUID REFERENCES users (id) ON DELETE SET NULL,
    sort          INT NOT NULL DEFAULT 100,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX departments_parent_idx ON departments (parent_id);

CREATE TABLE user_departments (
    user_id        UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    department_id  UUID NOT NULL REFERENCES departments (id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, department_id)
);
CREATE INDEX user_departments_dept_idx ON user_departments (department_id);

CREATE TABLE roles (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code         TEXT NOT NULL,
    name         TEXT NOT NULL,
    description  TEXT,
    is_system    BOOLEAN NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX roles_code_uidx ON roles (code);

CREATE TABLE role_permissions (
    role_id     UUID NOT NULL REFERENCES roles (id) ON DELETE CASCADE,
    permission  TEXT NOT NULL,
    PRIMARY KEY (role_id, permission)
);

CREATE TABLE user_roles (
    user_id  UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role_id  UUID NOT NULL REFERENCES roles (id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, role_id)
);
CREATE INDEX user_roles_role_idx ON user_roles (role_id);

CREATE TABLE settings (
    key         TEXT PRIMARY KEY,
    value       JSONB NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE audit_log (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_id     UUID REFERENCES users (id) ON DELETE SET NULL,
    action       TEXT NOT NULL,
    entity_type  TEXT NOT NULL,
    entity_id    TEXT,
    details      JSONB NOT NULL DEFAULT '{}'::jsonb,
    ip           TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_created_idx ON audit_log (created_at DESC);
CREATE INDEX audit_log_entity_idx ON audit_log (entity_type, entity_id);

-- Built-in roles. "admin" holds the wildcard permission; "employee" is assigned to every new user.
INSERT INTO roles (code, name, description, is_system) VALUES
    ('admin', 'Administrator', 'Full access to every part of NERP', TRUE),
    ('employee', 'Employee', 'Default role for every member of the company', TRUE);

INSERT INTO role_permissions (role_id, permission)
    SELECT id, '*' FROM roles WHERE code = 'admin';
INSERT INTO role_permissions (role_id, permission)
    SELECT id, 'users.view' FROM roles WHERE code = 'employee';
