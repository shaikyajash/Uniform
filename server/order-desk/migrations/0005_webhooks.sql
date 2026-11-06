-- B6: webhooks to shops. The Notifier writes one row per order and event when an order reaches a
-- final status, then delivers it with retries. The dashboard reads this as the delivery log.
CREATE TABLE webhook_log (
    id              BIGINT  GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    merchant_id     BIGINT  NOT NULL REFERENCES merchants (id),
    order_id        bytes32 NOT NULL REFERENCES orders (id),
    -- SPEC: order.paid, order.refunded, order.expired (a cross-chain order whose money went back
    -- to the customer is order.expired too: the shop was not paid).
    event           TEXT    NOT NULL CHECK (event IN ('order.paid', 'order.refunded', 'order.expired')),
    url             TEXT    NOT NULL,
    -- Exactly the JSON that is sent and signed.
    body            TEXT    NOT NULL,
    -- pending -> delivered, or failed after the last retry.
    status          TEXT    NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'delivered', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at BIGINT  NOT NULL,
    last_status     INTEGER,
    last_error      TEXT,
    created_at      BIGINT  NOT NULL,
    delivered_at    BIGINT,
    UNIQUE (order_id, event)
);

CREATE INDEX webhook_log_due ON webhook_log (next_attempt_at) WHERE status = 'pending';
CREATE INDEX webhook_log_by_merchant ON webhook_log (merchant_id, id);
