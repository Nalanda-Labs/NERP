-- A campaign's message designed in blocks (the email designer); its plain text goes in body.
ALTER TABLE mkt_campaigns ADD COLUMN design JSONB;
