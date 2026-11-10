-- Gmail and Microsoft 365 mailboxes sign in with OAuth2 (XOAUTH2 for IMAP and SMTP)
-- instead of a password. `secret` then holds the encrypted refresh token; the short-lived
-- access token is cached (encrypted) with its expiry.
ALTER TABLE crm_mailboxes ADD COLUMN auth_kind TEXT NOT NULL DEFAULT 'password' CHECK (auth_kind IN ('password', 'google', 'microsoft'));
ALTER TABLE crm_mailboxes ADD COLUMN access_token BYTEA;
ALTER TABLE crm_mailboxes ADD COLUMN access_expires_at TIMESTAMPTZ;
