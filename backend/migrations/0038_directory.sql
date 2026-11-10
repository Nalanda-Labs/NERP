-- Directory and single sign-on: employees from an LDAP directory (Active Directory,
-- OpenLDAP…) and sign-in through an OpenID Connect provider.

-- Where an account comes from: local (NERP password), ldap (the directory checks the
-- password), sso (signs in through the identity provider).
ALTER TABLE users ADD COLUMN auth_source TEXT NOT NULL DEFAULT 'local' CHECK (auth_source IN ('local', 'ldap', 'sso'));
-- The directory's or provider's id for the person, and (LDAP) their entry.
ALTER TABLE users ADD COLUMN external_id TEXT;
ALTER TABLE users ADD COLUMN ldap_dn TEXT;
CREATE UNIQUE INDEX users_external_idx ON users (auth_source, external_id) WHERE external_id IS NOT NULL;

-- One row: how employees are looked up in a company directory (LDAP) and signed in with
-- an identity provider (OpenID Connect). Secrets are encrypted with the data key.
CREATE TABLE directory_config (
    id          INT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    ldap        JSONB NOT NULL DEFAULT '{}',
    ldap_secret BYTEA,
    ldap_synced_at TIMESTAMPTZ,
    ldap_sync_result JSONB,
    sso         JSONB NOT NULL DEFAULT '{}',
    sso_secret  BYTEA
);
INSERT INTO directory_config (id) VALUES (1);

-- Deactivated because the person left the directory (a later sync turns them back on if
-- they reappear; employees an administrator deactivated stay off).
ALTER TABLE users ADD COLUMN directory_deactivated BOOLEAN NOT NULL DEFAULT FALSE;
