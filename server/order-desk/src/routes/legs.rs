//! `POST /v1/orders/{id}/legs`: the pay page sends the customer's signature on a cross-chain quote.
//!
//! The Order Desk checks the signature, records the leg it opens and writes a `source_lock` job
//! for the Relayer, which locks the customer's USDC for Bob and pays the gas. The order becomes
//! `paying`; the `Locked` event moves it on.

use alloy::primitives::{Address, B256};
use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
};
use common::{jobs::Job, status::OrderStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AppState, direct::recover_check, normalize_order_id, now, quote::PlannedLeg};
use crate::error::ApiError;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegsRequest {
    quote_id: String,
    /// 65 bytes (r, s, v) as 0x hex, from the wallet's `signTypedData` on the quote's note.
    signature: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegsAccepted {
    status: OrderStatus,
    job_id: i64,
    lock_id: B256,
}

#[derive(sqlx::FromRow)]
struct QuotedLeg {
    payer: String,
    typed_data: Value,
    valid_after: i64,
    valid_before: i64,
    expires_at: i64,
    leg: Option<Value>,
    status: String,
    merchant_id: i64,
}

pub async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<LegsRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<LegsAccepted>), ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let order_id = normalize_order_id(&id).ok_or_else(|| ApiError::not_found("order"))?;

    let quote: QuotedLeg = sqlx::query_as(
        "SELECT q.payer, q.typed_data, q.valid_after, q.valid_before, q.expires_at, q.leg, o.status, o.merchant_id
         FROM quotes q JOIN orders o ON o.id = q.order_id
         WHERE q.id = $1 AND q.order_id = $2 AND q.path = 'crosschain'",
    )
    .bind(&body.quote_id)
    .bind(&order_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("quote"))?;

    let status: OrderStatus = quote.status.parse().map_err(ApiError::internal)?;
    if status == OrderStatus::Paying {
        return Err(ApiError::conflict("already_submitted", "this order's payment is already being sent"));
    }
    if !status.can_move(OrderStatus::Paying) {
        return Err(ApiError::conflict("order_not_payable", format!("order is {status}")));
    }
    if now() >= quote.expires_at {
        return Err(ApiError::conflict("quote_expired", "the quote ran out; ask for a new one and sign again"));
    }
    let leg: PlannedLeg = serde_json::from_value(quote.leg.ok_or_else(|| ApiError::internal("cross-chain quote without a leg"))?)
        .map_err(ApiError::internal)?;
    let payer: Address = quote.payer.parse().map_err(ApiError::internal)?;
    let signature = recover_check(&body.signature, &quote.typed_data, payer)?;

    let job = Job::SourceLock {
        params: leg.params,
        payer,
        token: leg.token,
        amount: leg.amount,
        valid_after: u64::try_from(quote.valid_after).map_err(ApiError::internal)?,
        valid_before: u64::try_from(quote.valid_before).map_err(ApiError::internal)?,
        lock_id: leg.lock_id,
        signature,
    };
    let chain_id = i64::try_from(leg.chain_id).map_err(ApiError::internal)?;
    let now = now();

    let mut tx = state.pool.begin().await?;
    // Only from `quoted`, checked again here: a double click cannot make two jobs. The hashlock is
    // the signed quote's, in case a newer quote with another one was asked for in between.
    let moved = sqlx::query("UPDATE orders SET status = 'paying', hashlock = $2 WHERE id = $1 AND status = 'quoted'")
        .bind(&order_id)
        .bind(leg.params.hashlock.to_string())
        .execute(&mut *tx)
        .await?;
    if moved.rows_affected() == 0 {
        return Err(ApiError::conflict("already_submitted", "this order's payment is already being sent"));
    }
    // The same quote may be signed again after its lock job failed: the leg is already there.
    sqlx::query(
        "INSERT INTO legs (order_id, quote_id, chain_id, token, amount, payer, lock_id, expiry, created_at)
         VALUES ($1, $2, $3, $4, $5::numeric, $6, $7, $8, $9)
         ON CONFLICT (chain_id, lock_id) DO NOTHING",
    )
    .bind(&order_id)
    .bind(&body.quote_id)
    .bind(chain_id)
    .bind(format!("{:#x}", leg.token))
    .bind(leg.amount.to_string())
    .bind(format!("{payer:#x}"))
    .bind(leg.lock_id.to_string())
    .bind(i64::try_from(leg.params.expiry).map_err(ApiError::internal)?)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let job_id = super::insert_job(&mut tx, &order_id, quote.merchant_id, chain_id, &job, now).await?;
    tx.commit().await?;

    Ok((StatusCode::ACCEPTED, Json(LegsAccepted { status: OrderStatus::Paying, job_id, lock_id: leg.lock_id })))
}

#[cfg(test)]
pub(crate) mod tests {
    use alloy::{
        dyn_abi::TypedData,
        hex,
        signers::{SignerSync, local::PrivateKeySigner},
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use common::jobs::Job;
    use serde_json::{Value, json};
    use sqlx::PgPool;

    use crate::routes::test_support::{TestApp, call, test_app};

    pub const SECRET: &str = "0x0707070707070707070707070707070707070707070707070707070707070707";

    pub fn post(path: String, body: Value) -> Request<Body> {
        Request::post(path).header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap()
    }

    /// A 12 USDT order with a cross-chain quote for `payer` under SECRET's hashlock.
    pub async fn quoted(test: &TestApp, payer: &PrivateKeySigner) -> (String, Value) {
        let (_, created) = test.create_order(json!({ "amount": "12", "reference": "shop-123" })).await;
        let id = created["orderId"].as_str().unwrap().to_owned();
        let hashlock = common::locks::hashlock_of(SECRET.parse().unwrap());
        let body = json!({ "payer": payer.address(), "hashlock": hashlock, "chainId": 84532 });
        let (status, quote) = call(&test.app, post(format!("/v1/orders/{id}/quote"), body)).await;
        assert_eq!(status, StatusCode::OK, "{quote}");
        (id, quote)
    }

    pub fn sign(signer: &PrivateKeySigner, quote: &Value) -> String {
        let typed: TypedData = serde_json::from_value(quote["toSign"][0]["typedData"].clone()).unwrap();
        let signature = signer.sign_hash_sync(&typed.eip712_signing_hash().unwrap()).unwrap();
        format!("0x{}", hex::encode(signature.as_bytes()))
    }

    pub fn submit(id: &str, quote: &Value, signature: &str) -> Request<Body> {
        post(format!("/v1/orders/{id}/legs"), json!({ "quoteId": quote["quoteId"], "signature": signature }))
    }

    #[sqlx::test]
    async fn a_signed_quote_becomes_a_leg_and_one_lock_job(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(&test, &alice).await;

        let (status, reply) = call(&test.app, submit(&id, &quote, &sign(&alice, &quote))).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{reply}");
        assert_eq!(reply["status"], "paying");
        assert_eq!(reply["lockId"], quote["toSign"][0]["lockId"]);

        let (chain_id, amount, lock_id, leg_status): (i64, String, String, String) =
            sqlx::query_as("SELECT chain_id, amount::text, lock_id, status FROM legs WHERE order_id = $1")
                .bind(&id)
                .fetch_one(&test.pool)
                .await
                .unwrap();
        assert_eq!((chain_id, amount.as_str(), leg_status.as_str()), (84532, "12036000", "pending"));
        assert_eq!(lock_id, quote["toSign"][0]["lockId"].as_str().unwrap());

        let (kind, job_chain, args): (String, i64, Value) =
            sqlx::query_as("SELECT kind, chain_id, args FROM relayer_jobs WHERE id = $1")
                .bind(reply["jobId"].as_i64().unwrap())
                .fetch_one(&test.pool)
                .await
                .unwrap();
        assert_eq!((kind.as_str(), job_chain), ("source_lock", 84532));
        let Job::SourceLock { payer, amount, lock_id, params, valid_before, .. } = serde_json::from_value(args).unwrap() else {
            panic!("expected a source_lock job");
        };
        let message = &quote["toSign"][0]["typedData"]["message"];
        assert_eq!(payer, alice.address());
        assert_eq!(amount.to_string(), "12036000");
        assert_eq!(lock_id.to_string(), message["nonce"].as_str().unwrap());
        assert_eq!(valid_before.to_string(), message["validBefore"].as_str().unwrap());
        assert_eq!(params.receiver, crate::routes::test_support::PROVIDER);

        // The pay page and the Matchmaker see the exact lock id to expect.
        let read = Request::get(format!("/v1/orders/{id}")).body(Body::empty()).unwrap();
        let (_, order) = call(&test.app, read).await;
        assert_eq!(order["path"], "crosschain");
        assert_eq!(order["legs"][0]["lockId"], quote["toSign"][0]["lockId"]);
        assert_eq!((order["legs"][0]["chainId"].as_u64(), order["legs"][0]["status"].as_str()), (Some(84532), Some("pending")));
        assert_eq!(order["payment"]["status"], "pending");
        assert_eq!(order["hashlock"], common::locks::hashlock_of(SECRET.parse().unwrap()).to_string());
    }

    #[sqlx::test]
    async fn wrong_signers_doubles_and_path_a_quotes_are_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let (alice, mallory) = (PrivateKeySigner::random(), PrivateKeySigner::random());
        let (id, quote) = quoted(&test, &alice).await;

        let (status, reply) = call(&test.app, submit(&id, &quote, &sign(&mallory, &quote))).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_signature")));

        let signature = sign(&alice, &quote);
        assert_eq!(call(&test.app, submit(&id, &quote, &signature)).await.0, StatusCode::ACCEPTED);
        let (status, reply) = call(&test.app, submit(&id, &quote, &signature)).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("already_submitted")));
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM relayer_jobs").fetch_one(&test.pool).await.unwrap();
        assert_eq!(jobs, 1);

        // A Path A quote cannot be sent here.
        let (_, created) = test.create_order(json!({ "amount": "5", "reference": "shop-124" })).await;
        let other = created["orderId"].as_str().unwrap().to_owned();
        let (_, direct) = call(&test.app, post(format!("/v1/orders/{other}/quote"), json!({ "payer": alice.address() }))).await;
        assert_eq!(call(&test.app, submit(&other, &direct, &sign(&alice, &direct))).await.0, StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn an_expired_quote_is_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(&test, &alice).await;
        sqlx::query("UPDATE quotes SET expires_at = 1").execute(&test.pool).await.unwrap();
        let (status, reply) = call(&test.app, submit(&id, &quote, &sign(&alice, &quote))).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("quote_expired")));
    }
}
