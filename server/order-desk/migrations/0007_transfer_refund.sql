-- Shop refunds of cross-chain and split payments: the shop's USDT goes to the customer with a
-- signed transfer on the shop chain (`transfer_refund`), however the customer paid.
ALTER TABLE relayer_jobs DROP CONSTRAINT relayer_jobs_kind_check;
ALTER TABLE relayer_jobs ADD CONSTRAINT relayer_jobs_kind_check
    CHECK (kind IN ('direct_pay', 'source_lock', 'destination_claim', 'source_refund', 'direct_refund', 'transfer_refund'));
