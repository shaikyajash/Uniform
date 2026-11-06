//! `POST /v1/orders` (the shop's server makes a checkout) and `GET /v1/orders/{id}` (the pay page
//! and the shop read it).

use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
};
use common::{
    amount::Amount,
    jobs::JobStatus,
    status::{OrderStatus, PayPath},
};
use serde::{Deserialize, Serialize};

use super::{AppState, normalize_order_id, now};
use crate::{auth::Shop, error::ApiError, random};

/// USDT on Whitechain has 6 decimals: "12.000000" is 12 USDT.
const USDT_DECIMALS: u8 = 6;
/// How long the customer has to pay. Not in the plan; long enough for Path B's 20-minute lock.
const ORDER_LIFETIME_SECONDS: i64 = 30 * 60;
const MAX_REFERENCE_LENGTH: usize = 200;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateOrder {
    /// USDT as decimal text, at most 6 decimals: "12" or "12.50".
    amount: String,
    /// The shop's own id for this sale, sent back in webhooks.
    reference: String,
    success_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderCreated {
    order_id: String,
    status: OrderStatus,
    amount: String,
    pay_url: String,
    expires_at: i64,
}

pub async fn create(
    State(state): State<AppState>,
    shop: Shop,
    body: Result<Json<CreateOrder>, JsonRejection>,
) -> Result<(StatusCode, Json<OrderCreated>), ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let amount = Amount::from_decimal(&body.amount, USDT_DECIMALS).map_err(|_| {
        ApiError::bad_request("bad_amount", "amount must be USDT as decimal text with at most 6 decimals, like \"12.50\"")
    })?;
    if amount == Amount::default() {
        return Err(ApiError::bad_request("bad_amount", "amount must be above zero"));
    }
    // The Relayer refuses to pay gas above this, so such an order could never be paid.
    if amount > state.max_order {
        let limit = state.max_order.to_decimal(USDT_DECIMALS);
        return Err(ApiError::bad_request("amount_too_large", format!("amount must be at most {limit} USDT")));
    }
    let reference = body.reference.trim();
    if reference.is_empty() || reference.len() > MAX_REFERENCE_LENGTH {
        return Err(ApiError::bad_request("bad_reference", "reference must be 1 to 200 characters"));
    }
    if let Some(url) = &body.success_url
        && !(url.starts_with("https://") || url.starts_with("http://"))
    {
        return Err(ApiError::bad_request("bad_success_url", "successUrl must start with https:// or http://"));
    }

    // Random, not derived from the shop or a counter: reading an order needs only its id.
    let order_id = random::bytes32_hex();
    let created_at = now();
    let expires_at = created_at + ORDER_LIFETIME_SECONDS;
    sqlx::query(
        "INSERT INTO orders (id, merchant_id, amount, reference, success_url, created_at, expires_at)
         VALUES ($1, $2, $3::numeric, $4, $5, $6, $7)",
    )
    .bind(&order_id)
    .bind(shop.merchant_id)
    .bind(amount.to_string())
    .bind(reference)
    .bind(&body.success_url)
    .bind(created_at)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;

    let pay_url = format!("{}/pay/{order_id}", state.pay_base_url.trim_end_matches('/'));
    let created = OrderCreated {
        order_id,
        status: OrderStatus::Created,
        amount: amount.to_decimal(USDT_DECIMALS),
        pay_url,
        expires_at,
    };
    Ok((StatusCode::CREATED, Json(created)))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderView {
    order_id: String,
    shop: ShopView,
    reference: String,
    amount: String,
    status: OrderStatus,
    path: Option<PayPath>,
    payer: Option<String>,
    created_at: i64,
    expires_at: i64,
    paid_at: Option<i64>,
    /// The Relayer's latest attempt to send the customer's payment (Path A) or lock (cross-chain).
    payment: Option<PaymentView>,
    /// Cross-chain: `keccak256(secret)` of the signed quote.
    hashlock: Option<String>,
    /// Cross-chain: the customer's locks, from the latest signed quote.
    legs: Vec<LegView>,
    /// Cross-chain: Bob's lock for the shop on Whitechain, once filled (its id was recomputed).
    destination_lock_id: Option<String>,
    /// Cross-chain: the Relayer's latest attempt to claim the shop's lock with the secret.
    claim: Option<PaymentView>,
}

/// One customer lock. `lockId` is the only id to trust: SourceLock's `lockIdFor` of what was signed.
#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct LegView {
    #[sqlx(try_from = "i64")]
    chain_id: u64,
    token: String,
    /// Smallest units (6 decimals for USDC).
    amount: String,
    lock_id: String,
    expiry: i64,
    status: String,
    lock_tx: Option<String>,
    claim_tx: Option<String>,
    refund_tx: Option<String>,
}

/// What the pay page shows while the Relayer works: `pending -> sending -> sent -> confirmed`, or
/// `failed`. `confirmed` means the transaction succeeded on chain; the order itself becomes `paid`
/// when the `Paid` event is reported.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentView {
    status: JobStatus,
    /// Only once a node has the transaction (`sent` and later).
    tx_hash: Option<String>,
    explorer_url: Option<String>,
    /// Only when `failed`: why, in words the customer can act on.
    failed_reason: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PaymentRow {
    status: String,
    tx_hash: Option<String>,
    last_error: Option<String>,
    chain_id: i64,
}

#[derive(Debug, Serialize)]
pub struct ShopView {
    name: String,
    address: String,
}

#[derive(sqlx::FromRow)]
struct OrderRow {
    id: String,
    shop_name: String,
    shop_address: String,
    reference: String,
    amount: String,
    status: String,
    path: Option<String>,
    payer: Option<String>,
    created_at: i64,
    expires_at: i64,
    paid_at: Option<i64>,
    hashlock: Option<String>,
    destination_lock_id: Option<String>,
}

/// Public: the pay page has no login. The random 32-byte id is what keeps orders private.
pub async fn read(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<OrderView>, ApiError> {
    let id = normalize_order_id(&id).ok_or_else(|| ApiError::not_found("order"))?;
    let row: Option<OrderRow> = sqlx::query_as(
        "SELECT o.id, m.name AS shop_name, m.payout_address AS shop_address, o.reference,
                o.amount::text AS amount, o.status, o.path, o.payer, o.created_at, o.expires_at, o.paid_at,
                o.hashlock, o.destination_lock_id
         FROM orders o JOIN merchants m ON m.id = o.merchant_id
         WHERE o.id = $1",
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await?;
    let row = row.ok_or_else(|| ApiError::not_found("order"))?;

    let amount: Amount = row.amount.parse().map_err(ApiError::internal)?;
    let status: OrderStatus = row.status.parse().map_err(ApiError::internal)?;
    let path = row.path.map(|path| path.parse::<PayPath>()).transpose().map_err(ApiError::internal)?;
    let payment = latest_job(&state, &id, &["direct_pay", "source_lock"]).await?;
    let claim = latest_job(&state, &id, &["destination_claim"]).await?;
    let legs: Vec<LegView> = sqlx::query_as(
        "SELECT chain_id, token, amount::text AS amount, lock_id, expiry, status, lock_tx, claim_tx, refund_tx
         FROM legs
         WHERE quote_id = (SELECT quote_id FROM legs WHERE order_id = $1 ORDER BY id DESC LIMIT 1)
         ORDER BY id",
    )
    .bind(&id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(OrderView {
        order_id: row.id,
        shop: ShopView { name: row.shop_name, address: row.shop_address },
        reference: row.reference,
        amount: amount.to_decimal(USDT_DECIMALS),
        status,
        path,
        payer: row.payer,
        created_at: row.created_at,
        expires_at: row.expires_at,
        paid_at: row.paid_at,
        payment,
        hashlock: row.hashlock,
        legs,
        destination_lock_id: row.destination_lock_id,
        claim,
    }))
}

/// The latest Relayer job of these kinds for the order, as the pay page shows it.
async fn latest_job(state: &AppState, order_id: &str, kinds: &[&str]) -> Result<Option<PaymentView>, ApiError> {
    let row: Option<PaymentRow> = sqlx::query_as(
        "SELECT status, tx_hash, last_error, chain_id FROM relayer_jobs
         WHERE order_id = $1 AND kind = ANY($2) ORDER BY id DESC LIMIT 1",
    )
    .bind(order_id)
    .bind(kinds)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let status: JobStatus = row.status.parse().map_err(ApiError::internal)?;
    // `sending` has a hash that may never reach a node; show it once a node accepted it.
    let tx_hash = row.tx_hash.filter(|_| matches!(status, JobStatus::Sent | JobStatus::Confirmed));
    let chain = u64::try_from(row.chain_id).ok().and_then(|id| state.chains.by_id(id));
    let explorer = chain.unwrap_or_else(|| state.chains.shop_chain()).explorer.trim_end_matches('/');
    Ok(Some(PaymentView {
        status,
        explorer_url: tx_hash.as_ref().map(|hash| format!("{explorer}/tx/{hash}")),
        tx_hash,
        failed_reason: row.last_error.filter(|_| status == JobStatus::Failed),
    }))
}


#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use serde_json::{Value, json};
    use sqlx::PgPool;

    use crate::{
        auth::new_api_key,
        routes::test_support::{call, test_app},
    };

    fn create(key: Option<&str>, body: Value) -> Request<Body> {
        let mut request = Request::post("/v1/orders").header(header::CONTENT_TYPE, "application/json");
        if let Some(key) = key {
            request = request.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        request.body(Body::from(body.to_string())).unwrap()
    }

    fn read(id: &str) -> Request<Body> {
        Request::get(format!("/v1/orders/{id}")).body(Body::empty()).unwrap()
    }

    #[sqlx::test]
    async fn a_shop_creates_an_order_and_anyone_with_the_id_reads_it(pool: PgPool) {
        let test = test_app(pool).await;
        let (status, created) = call(
            &test.app,
            create(Some(&test.key), json!({ "amount": "12", "reference": "shop-123", "successUrl": "https://kettle.example/thanks" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let id = created["orderId"].as_str().unwrap();
        assert_eq!(created["status"], "created");
        assert_eq!(created["amount"], "12.000000");
        assert_eq!(created["payUrl"], format!("https://pay.example/pay/{id}"));

        let stored: String = sqlx::query_scalar("SELECT amount::text FROM orders WHERE id = $1")
            .bind(id)
            .fetch_one(&test.pool)
            .await
            .unwrap();
        assert_eq!(stored, "12000000");

        let (status, order) = call(&test.app, read(&id.to_ascii_uppercase().replacen("0X", "0x", 1))).await;
        assert_eq!(status, StatusCode::OK, "{order}");
        assert_eq!(order["orderId"], id);
        assert_eq!(order["shop"], json!({ "name": "Kettle & Co", "address": "0x5a30000000000000000000000000000000000001" }));
        assert_eq!(order["reference"], "shop-123");
        assert_eq!(order["amount"], "12.000000");
        assert_eq!(order["status"], "created");
        assert_eq!(order["path"], Value::Null);
        assert_eq!((order["legs"].clone(), order["hashlock"].clone()), (json!([]), Value::Null));
    }

    #[sqlx::test]
    async fn order_ids_are_random(pool: PgPool) {
        let test = test_app(pool).await;
        let body = json!({ "amount": "1", "reference": "same" });
        let (_, first) = call(&test.app, create(Some(&test.key), body.clone())).await;
        let (_, second) = call(&test.app, create(Some(&test.key), body)).await;
        assert_ne!(first["orderId"], second["orderId"]);
    }

    #[sqlx::test]
    async fn creating_needs_a_valid_api_key(pool: PgPool) {
        let test = test_app(pool).await;
        let body = json!({ "amount": "12", "reference": "r" });
        let (stranger, _) = new_api_key();
        for key in [None, Some(stranger.as_str())] {
            let (status, reply) = call(&test.app, create(key, body.clone())).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(reply["error"]["code"], "unauthorized");
        }
    }

    #[sqlx::test]
    async fn bad_input_gets_a_400_with_a_code(pool: PgPool) {
        let test = test_app(pool).await;
        for (body, code) in [
            (json!({ "amount": "0", "reference": "r" }), "bad_amount"),
            (json!({ "amount": "12.0000001", "reference": "r" }), "bad_amount"),
            (json!({ "amount": "100.000001", "reference": "r" }), "amount_too_large"),
            (json!({ "amount": 12, "reference": "r" }), "bad_json"),
            (json!({ "amount": "12", "reference": "  " }), "bad_reference"),
            (json!({ "amount": "12", "reference": "r", "successUrl": "javascript:alert(1)" }), "bad_success_url"),
            (json!({ "amount": "12", "reference": "r", "currency": "EUR" }), "bad_json"),
        ] {
            let (status, reply) = call(&test.app, create(Some(&test.key), body.clone())).await;
            assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{body}");
        }
    }

    #[sqlx::test]
    async fn unknown_or_malformed_ids_are_404(pool: PgPool) {
        let test = test_app(pool).await;
        let unknown = format!("0x{}", "cd".repeat(32));
        for id in [unknown.as_str(), "0x12", "1001", "0x'%20OR%201=1%20--"] {
            let (status, reply) = call(&test.app, read(id)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{id}");
            assert_eq!(reply["error"]["code"], "not_found");
        }
    }
}
