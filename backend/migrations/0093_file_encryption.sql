-- Encryption of stored files at rest: the key a file was written with (NULL: not encrypted).
ALTER TABLE files ADD COLUMN enc_key text;
CREATE INDEX files_enc_key ON files (enc_key);
