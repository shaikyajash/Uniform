-- B7: the shop dashboard. A shop signs in with its payout wallet (a signed one-time message, then
-- a session), and refunds Path A payments through a fifth Relayer job.

-- One sign-in message per attempt. Used once, and only before it expires.
CREATE TABLE login_challenges (
    nonce      TEXT    PRIMARY KEY,
    address    address NOT NULL,
    message    TEXT    NOT NULL,
    expires_at BIGINT  NOT NULL,
    used       BOOLEAN NOT NULL DEFAULT FALSE
);

-- A signed-in dashboard. Only the sha256 of the token is stored, as for API keys.
CREATE TABLE sessions (
    token_hash  TEXT   PRIMARY KEY,
    merchant_id BIGINT NOT NULL REFERENCES merchants (id),
    created_at  BIGINT NOT NULL,
    expires_at  BIGINT NOT NULL
);

CREATE INDEX sessions_by_merchant ON sessions (merchant_id);

ALTER TABLE relayer_jobs DROP CONSTRAINT relayer_jobs_kind_check;
ALTER TABLE relayer_jobs ADD CONSTRAINT relayer_jobs_kind_check
    CHECK (kind IN ('direct_pay', 'source_lock', 'destination_claim', 'source_refund', 'direct_refund'));
