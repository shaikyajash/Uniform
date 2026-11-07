-- Order Desk tables used by B1 (Postgres). Later tasks add theirs in their own migration:
-- relayer jobs and gas budget (B2), legs (B4), webhook_log (B6), refunds and links (B7).
--
-- Conventions
--   addresses and 32-byte ids: lowercase 0x hex text, checked by the domains below
--   amounts: NUMERIC(78, 0), whole smallest units ("12000000" = 12 USDT); 78 digits fit any
--            uint256, and SQL can compare and add them. Read as amount::text, write as $1::numeric
--   times: unix seconds (BIGINT), the same as the chains and the API

CREATE DOMAIN address AS TEXT CHECK (VALUE ~ '^0x[0-9a-f]{40}$');
CREATE DOMAIN bytes32 AS TEXT CHECK (VALUE ~ '^0x[0-9a-f]{64}$');
CREATE DOMAIN amount  AS NUMERIC(78, 0) CHECK (VALUE >= 0);

-- A shop. It signs in with its payout wallet; refunds are signed by that same wallet.
CREATE TABLE merchants (
    id             BIGINT  GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name           TEXT    NOT NULL,
    payout_address address NOT NULL UNIQUE,
    -- sha256 of the API key. The key itself is shown once and never stored.
    api_key_hash   TEXT    UNIQUE,
    webhook_url    TEXT,
    -- Kept as is: the Notifier needs it to sign every webhook.
    webhook_secret TEXT    NOT NULL,
    created_at     BIGINT  NOT NULL
);

-- One checkout. The status only moves along common::status::OrderStatus::next().
CREATE TABLE orders (
    id          bytes32 PRIMARY KEY,
    merchant_id BIGINT  NOT NULL REFERENCES merchants (id),
    amount      amount  NOT NULL CHECK (amount > 0),
    reference   TEXT    NOT NULL,
    success_url TEXT,
    status      TEXT    NOT NULL DEFAULT 'created' CHECK (status IN (
                    'created', 'quoted', 'paying', 'source_locked', 'destination_locked',
                    'paid', 'partially_refunded', 'refunded', 'expired', 'returned')),
    path        TEXT    CHECK (path IN ('direct', 'crosschain', 'split')),
    payer       address,
    hashlock    bytes32,
    created_at  BIGINT  NOT NULL,
    -- The customer must pay before this, or the order expires.
    expires_at  BIGINT  NOT NULL,
    paid_at     BIGINT
);

CREATE INDEX orders_by_merchant ON orders (merchant_id, created_at);

-- What the customer was asked to sign. The relayer only submits a signature that matches one.
CREATE TABLE quotes (
    id           TEXT    PRIMARY KEY,
    order_id     bytes32 NOT NULL REFERENCES orders (id),
    path         TEXT    NOT NULL CHECK (path IN ('direct', 'crosschain', 'split')),
    payer        address NOT NULL,
    -- The exact EIP-712 JSON sent to the pay page.
    typed_data   JSONB   NOT NULL,
    -- EIP-3009 nonce in the note: DirectPay.paymentIdFor(...) on Path A.
    nonce        bytes32 NOT NULL,
    valid_after  BIGINT  NOT NULL,
    valid_before BIGINT  NOT NULL,
    expires_at   BIGINT  NOT NULL,
    created_at   BIGINT  NOT NULL
);

CREATE INDEX quotes_by_order ON quotes (order_id, created_at);

-- Every chain event reported on /internal/events (common::events::ChainEvent).
-- The primary key makes a second report of the same event a no-op (ON CONFLICT DO NOTHING).
-- Money facts come only from this table; order status can be rebuilt from it.
CREATE TABLE events (
    chain_id     BIGINT  NOT NULL,
    tx_hash      bytes32 NOT NULL,
    log_index    BIGINT  NOT NULL,
    block_number BIGINT  NOT NULL,
    name         TEXT    NOT NULL,
    -- Claimed and Refunded carry only a lock id, so this can be empty.
    order_id     bytes32,
    -- The full ChainEvent.
    data         JSONB   NOT NULL,
    received_at  BIGINT  NOT NULL,
    PRIMARY KEY (chain_id, tx_hash, log_index)
);

CREATE INDEX events_by_order ON events (order_id);
