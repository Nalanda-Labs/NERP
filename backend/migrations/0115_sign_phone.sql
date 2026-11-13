-- A signer's phone: the signing link goes to it by text message as well as by email.
ALTER TABLE sign_signers ADD COLUMN phone TEXT;
