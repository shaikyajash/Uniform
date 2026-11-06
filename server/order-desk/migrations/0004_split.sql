-- A8: split payments. A split order has one source lock per chain, so it can have a lock job (and
-- later a refund job) running on each chain at once. Still one unfinished job per order, kind and
-- chain: a double click cannot send twice.
DROP INDEX relayer_jobs_one_live_per_order_and_kind;
CREATE UNIQUE INDEX relayer_jobs_one_live_per_order_kind_and_chain
    ON relayer_jobs (order_id, kind, chain_id) WHERE status IN ('pending', 'sending', 'sent');
