-- B4: cross-chain payments. A quote can plan a source lock; a signed one becomes a leg; the Relayer
-- gets three more kinds of job (lock, destination claim, source refund).

-- The source lock a cross-chain quote asks the customer to sign for (JSON, see quote.rs):
-- chain, token, amount, lock params and the lock id. NULL on Path A quotes.
ALTER TABLE quotes ADD COLUMN leg JSONB;

-- Bob's lock for the shop on Whitechain, from a `Filled` whose id we recomputed.
ALTER TABLE orders ADD COLUMN destination_lock_id bytes32;

-- One customer lock on one chain. Only an exact lock id we planned can move a leg.
CREATE TABLE legs (
    id         BIGINT  GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    order_id   bytes32 NOT NULL REFERENCES orders (id),
    quote_id   TEXT    NOT NULL REFERENCES quotes (id),
    chain_id   BIGINT  NOT NULL,
    token      address NOT NULL,
    amount     amount  NOT NULL CHECK (amount > 0),
    payer      address NOT NULL,
    lock_id    bytes32 NOT NULL,
    -- The lock's expiry (unix seconds): refundable to the customer from then on.
    expiry     BIGINT  NOT NULL,
    -- common::status::LegStatus
    status     TEXT    NOT NULL DEFAULT 'pending'
               CHECK (status IN ('pending', 'locked', 'claimed', 'returned')),
    lock_tx    bytes32,
    claim_tx   bytes32,
    refund_tx  bytes32,
    created_at BIGINT  NOT NULL,
    UNIQUE (chain_id, lock_id)
);

CREATE INDEX legs_by_order ON legs (order_id, id);

ALTER TABLE relayer_jobs DROP CONSTRAINT relayer_jobs_kind_check;
ALTER TABLE relayer_jobs ADD CONSTRAINT relayer_jobs_kind_check
    CHECK (kind IN ('direct_pay', 'source_lock', 'destination_claim', 'source_refund'));

-- One unfinished job per order and kind: a double click cannot send twice, and a refund or a claim
-- can follow a finished lock job.
DROP INDEX relayer_jobs_one_live_per_order;
CREATE UNIQUE INDEX relayer_jobs_one_live_per_order_and_kind
    ON relayer_jobs (order_id, kind) WHERE status IN ('pending', 'sending', 'sent');
