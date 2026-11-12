-- SAML 2.0 single sign-on (src/directory/saml.rs): its settings and NERP's own key (sealed).
ALTER TABLE directory_config ADD COLUMN saml JSONB NOT NULL DEFAULT '{}';
ALTER TABLE directory_config ADD COLUMN saml_key BYTEA;
ALTER TABLE users DROP CONSTRAINT IF EXISTS users_auth_source_check;
ALTER TABLE users ADD CONSTRAINT users_auth_source_check CHECK (auth_source IN ('local', 'ldap', 'sso', 'saml'));
