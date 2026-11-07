-- Several providers: each cross-chain quote goes to the cheapest provider who can cover it. The
-- provider receives the customer's lock and pays the shop, so the quote records who it is, and the
-- order takes it from the quote the customer signs (also to count what each provider has been
-- promised by orders still being paid).
ALTER TABLE quotes ADD COLUMN provider_fee_bps INTEGER CHECK (provider_fee_bps BETWEEN 0 AND 9999);
ALTER TABLE orders ADD COLUMN provider address;
ALTER TABLE orders ADD COLUMN provider_fee_bps INTEGER CHECK (provider_fee_bps BETWEEN 0 AND 9999);
CREATE INDEX orders_by_provider ON orders (provider) WHERE provider IS NOT NULL;

-- Cross-chain orders already under way had one provider: the receiver in their newest quote's lock.
UPDATE orders o SET provider = lower(coalesce(q.leg -> 'params' ->> 'receiver', q.leg -> 0 -> 'params' ->> 'receiver'))
FROM (
    SELECT DISTINCT ON (order_id) order_id, leg FROM quotes WHERE leg IS NOT NULL ORDER BY order_id, created_at DESC
) q
WHERE q.order_id = o.id AND o.path IN ('crosschain', 'split');
