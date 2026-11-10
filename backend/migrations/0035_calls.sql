-- Video calls: peer-to-peer in the browser, signalled through NERP.

CREATE TABLE calls (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    chat_id    UUID REFERENCES im_chats (id) ON DELETE SET NULL,
    started_by UUID REFERENCES users (id) ON DELETE SET NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at   TIMESTAMPTZ
);

-- Who is invited; each time someone joins they get a fresh peer id.
CREATE TABLE call_invites (
    call_id  UUID NOT NULL REFERENCES calls (id) ON DELETE CASCADE,
    user_id  UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    declined BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (call_id, user_id)
);

CREATE TABLE call_peers (
    peer_id   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    call_id   UUID NOT NULL REFERENCES calls (id) ON DELETE CASCADE,
    user_id   UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Updated while the browser is in the call; a silent peer is dropped.
    seen_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    left_at   TIMESTAMPTZ
);
CREATE INDEX call_peers_call_idx ON call_peers (call_id ASC) WHERE left_at IS NULL;
