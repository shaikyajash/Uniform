//! Which orders owe their shop a webhook, and the exact JSON to send (SPEC "Webhook"):
//!
//! ```json
//! { "type": "order.paid", "orderId": "0x…", "reference": "shop-123", "amount": "12.000000",
//!   "currency": "USDT", "path": "direct",
//!   "receipts": { "customer": ["…tx url"], "settlement": "…tx url" } }
//! ```

use common::{amount::Amount, chains::Chains};
use serde_json::{Value, json};
use sqlx::PgPool;

/// Only orders this recent are sent, so a shop that sets its webhook URL late is not flooded
/// with old orders.
const RECENT_SECONDS: i64 = 24 * 3600;
const USDT_DECIMALS: u8 = 6;

/// The webhook event for an order status, if it has one. A cross-chain order whose money went
/// back to the customer is `order.expired` for the shop: it was not paid.
pub fn event_for(status: &str) -> Option<&'static str> {
    match status {
        "paid" => Some("order.paid"),
        "refunded" => Some("order.refunded"),
        "expired" | "returned" => Some("order.expired"),
        _ => None,
    }
}

#[derive(sqlx::FromRow)]
struct Due {
    id: String,
    merchant_id: i64,
    url: String,
    reference: String,
    amount: String,
    status: String,
    path: Option<String>,
}

/// Writes one `webhook_log` row per order and event still owed. Returns how many.
pub async fn queue(pool: &PgPool, chains: &Chains, now: i64) -> Result<u64, sqlx::Error> {
    let due: Vec<Due> = sqlx::query_as(
        "SELECT o.id, o.merchant_id, m.webhook_url AS url, o.reference, o.amount::text AS amount, o.status, o.path
         FROM orders o JOIN merchants m ON m.id = o.merchant_id
         WHERE m.webhook_url IS NOT NULL AND m.webhook_url <> ''
           AND o.status IN ('paid', 'refunded', 'expired', 'returned')
           AND o.created_at >= $1
           AND NOT EXISTS (
                 SELECT 1 FROM webhook_log w
                 WHERE w.order_id = o.id
                   AND w.event = CASE o.status WHEN 'paid' THEN 'order.paid' WHEN 'refunded' THEN 'order.refunded'
                                               ELSE 'order.expired' END)
         ORDER BY o.created_at
         LIMIT 100",
    )
    .bind(now - RECENT_SECONDS)
    .fetch_all(pool)
    .await?;

    let mut written = 0;
    for order in due {
        let Some(event) = event_for(&order.status) else { continue };
        let receipts = receipts(pool, chains, &order.id).await?;
        let amount = order.amount.parse::<Amount>().map(|amount| amount.to_decimal(USDT_DECIMALS)).unwrap_or(order.amount);
        let body = json!({
            "type": event,
            "orderId": order.id,
            "reference": order.reference,
            "amount": amount,
            "currency": "USDT",
            "path": order.path,
            "receipts": receipts,
        });
        written += sqlx::query(
            "INSERT INTO webhook_log (merchant_id, order_id, event, url, body, next_attempt_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $6)
             ON CONFLICT (order_id, event) DO NOTHING",
        )
        .bind(order.merchant_id)
        .bind(&order.id)
        .bind(event)
        .bind(&order.url)
        .bind(body.to_string())
        .bind(now)
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(written)
}

/// Explorer links: the customer's transaction(s) and the one that paid the shop. Path A is one
/// transaction (DirectPay moves the USDT straight to the shop). Cross-chain: the customer's lock
/// on their chain, and our claim (or anyone's `Settled`) on Whitechain.
async fn receipts(pool: &PgPool, chains: &Chains, order_id: &str) -> Result<Value, sqlx::Error> {
    let jobs: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT kind, chain_id, tx_hash FROM relayer_jobs WHERE order_id = $1 AND status = 'confirmed' ORDER BY id",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;
    let locks: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT chain_id, lock_tx FROM legs WHERE order_id = $1 AND lock_tx IS NOT NULL ORDER BY id")
            .bind(order_id)
            .fetch_all(pool)
            .await?;
    let settled: Option<(i64, String)> = sqlx::query_as(
        "SELECT chain_id, tx_hash FROM events WHERE order_id = $1 AND name IN ('Paid', 'Settled') ORDER BY received_at LIMIT 1",
    )
    .bind(order_id)
    .fetch_optional(pool)
    .await?;

    let url = |chain_id: i64, hash: &str| {
        let explorer = u64::try_from(chain_id).ok().and_then(|id| chains.by_id(id)).map(|chain| chain.explorer.as_str());
        explorer.map(|explorer| format!("{}/tx/{hash}", explorer.trim_end_matches('/')))
    };
    let job = |kind: &str| jobs.iter().find(|(k, _, hash)| k == kind && hash.is_some());

    let mut customer: Vec<String> = Vec::new();
    if let Some((_, chain, Some(hash))) = job("direct_pay") {
        customer.extend(url(*chain, hash));
    }
    for (chain, hash) in &locks {
        if let Some(hash) = hash {
            customer.extend(url(*chain, hash));
        }
    }
    if customer.is_empty()
        && let Some((_, chain, Some(hash))) = job("source_lock")
    {
        customer.extend(url(*chain, hash));
    }
    let settlement = job("destination_claim")
        .or_else(|| job("direct_pay"))
        .and_then(|(_, chain, hash)| hash.as_deref().and_then(|hash| url(*chain, hash)))
        .or_else(|| settled.and_then(|(chain, hash)| url(chain, &hash)));
    Ok(json!({ "customer": customer, "settlement": settlement }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_statuses_map_to_the_spec_events() {
        assert_eq!(event_for("paid"), Some("order.paid"));
        assert_eq!(event_for("refunded"), Some("order.refunded"));
        assert_eq!(event_for("expired"), Some("order.expired"));
        assert_eq!(event_for("returned"), Some("order.expired"));
        for status in ["created", "quoted", "paying", "source_locked", "destination_locked", "partially_refunded"] {
            assert_eq!(event_for(status), None, "{status}");
        }
    }

    const CHAINS: &str = include_str!("../../../shared/chains.json");

    async fn shop_with_order(pool: &PgPool, status: &str, url: Option<&str>) -> String {
        sqlx::query(
            "INSERT INTO merchants (name, payout_address, webhook_url, webhook_secret, created_at)
             VALUES ('Kettle & Co', '0x5a30000000000000000000000000000000000001', $1, 'whsec_test', 1)",
        )
        .bind(url)
        .execute(pool)
        .await
        .unwrap();
        let id = format!("0x{}", "ab".repeat(32));
        sqlx::query(
            "INSERT INTO orders (id, merchant_id, amount, reference, status, path, created_at, expires_at)
             SELECT $1, id, 12000000, 'shop-123', $2, 'direct', $3, $3 + 1800 FROM merchants",
        )
        .bind(&id)
        .bind(status)
        .bind(super::super::unix_now())
        .execute(pool)
        .await
        .unwrap();
        id
    }

    #[sqlx::test(migrations = "../order-desk/migrations")]
    async fn a_paid_order_gets_one_webhook_with_its_receipts(pool: PgPool) {
        let chains = Chains::from_json(CHAINS).unwrap();
        let id = shop_with_order(&pool, "paid", Some("https://shop.example/hook")).await;
        let hash = format!("0x{}", "cd".repeat(32));
        sqlx::query(
            "INSERT INTO relayer_jobs (order_id, merchant_id, chain_id, kind, args, status, tx_hash, created_at, updated_at)
             SELECT $1, id, 1874, 'direct_pay', '{}', 'confirmed', $2, 1, 1 FROM merchants",
        )
        .bind(&id)
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();

        let now = super::super::unix_now();
        assert_eq!(queue(&pool, &chains, now).await.unwrap(), 1);
        assert_eq!(queue(&pool, &chains, now).await.unwrap(), 0, "once per order and event");

        let body: String = sqlx::query_scalar("SELECT body FROM webhook_log").fetch_one(&pool).await.unwrap();
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["type"], "order.paid");
        assert_eq!((body["amount"].as_str(), body["currency"].as_str()), (Some("12.000000"), Some("USDT")));
        let tx = format!("https://explorer.testnet.whitechain.io/tx/{hash}");
        assert_eq!(body["receipts"], json!({ "customer": [tx.clone()], "settlement": tx }));
    }

    #[sqlx::test(migrations = "../order-desk/migrations")]
    async fn no_url_or_no_final_status_means_no_webhook(pool: PgPool) {
        let chains = Chains::from_json(CHAINS).unwrap();
        shop_with_order(&pool, "paid", None).await;
        assert_eq!(queue(&pool, &chains, super::super::unix_now()).await.unwrap(), 0);
        sqlx::query("UPDATE merchants SET webhook_url = 'https://shop.example/hook'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE orders SET status = 'paying'").execute(&pool).await.unwrap();
        assert_eq!(queue(&pool, &chains, super::super::unix_now()).await.unwrap(), 0);
    }
}
