-- B2: the hand-off between the Order Desk and the Relayer, and each shop's daily gas budget.
-- The Order Desk writes jobs; the Relayer takes, sends and finishes them. Neither calls the other.

-- One transaction the Relayer must send, as typed arguments (common::jobs::Job), never raw calldata.
CREATE TABLE relayer_jobs (
    id          BIGINT  GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    order_id    bytes32 NOT NULL REFERENCES orders (id),
    -- The shop whose gas budget pays for it.
    merchant_id BIGINT  NOT NULL REFERENCES merchants (id),
    chain_id    BIGINT  NOT NULL,
    kind        TEXT    NOT NULL CHECK (kind IN ('direct_pay')),
    args        JSONB   NOT NULL,
    -- pending -> sending (signed and saved) -> sent -> confirmed, or failed (common::jobs::JobStatus).
    status      TEXT    NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending', 'sending', 'sent', 'confirmed', 'failed')),
    -- Our signed transaction, saved BEFORE it is sent. After a timeout or a crash the same bytes
    -- are sent again (same nonce, same hash), so one job can never become two payments.
    tx_hash     bytes32,
    raw_tx      BYTEA,
    tx_nonce    BIGINT,
    -- When the current transaction was signed, and how often its fee was raised because it was
    -- not mined (same nonce, so only one of them can ever be mined).
    signed_at   BIGINT,
    fee_bumps   INTEGER NOT NULL DEFAULT 0,
    -- A Relayer that takes a job holds it until `lease_until`. `lease_token` is a random number
    -- for that holder; every update checks it, so a Relayer that lost the job writes nothing.
    lease_until BIGINT  NOT NULL DEFAULT 0,
    lease_token BIGINT  NOT NULL DEFAULT 0,
    attempts    INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT,
    -- From the receipt: gas units, and what they cost in the chain's coin (wei).
    gas_used    BIGINT,
    gas_cost    amount,
    created_at  BIGINT  NOT NULL,
    updated_at  BIGINT  NOT NULL,
    CHECK (status NOT IN ('sending', 'sent', 'confirmed') OR tx_hash IS NOT NULL),
    CHECK (status <> 'sending' OR raw_tx IS NOT NULL)
);

-- One unfinished job per order: a double click on "Pay" cannot send twice.
CREATE UNIQUE INDEX relayer_jobs_one_live_per_order
    ON relayer_jobs (order_id) WHERE status IN ('pending', 'sending', 'sent');

-- What the Relayer looks for: the oldest jobs still to do.
CREATE INDEX relayer_jobs_to_do ON relayer_jobs (created_at) WHERE status IN ('pending', 'sending', 'sent');

-- Gas the Relayer spent for each shop, per chain and UTC day (unix seconds / 86400).
-- The Relayer stops sending for a shop once `spent` reaches its daily limit.
CREATE TABLE gas_budget (
    merchant_id BIGINT NOT NULL REFERENCES merchants (id),
    chain_id    BIGINT NOT NULL,
    day         BIGINT NOT NULL,
    spent       amount NOT NULL DEFAULT 0,
    PRIMARY KEY (merchant_id, chain_id, day)
);
