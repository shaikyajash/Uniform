//! `POST /v1/orders/{id}/direct`: the pay page sends the customer's signature on a Path A quote.
//!
//! The Order Desk checks the signature, writes a `direct_pay` job for the Relayer and moves the
//! order to `paying`. It sends nothing itself and never marks the order paid: that comes from the
//! `Paid` event on `/internal/events`.

use alloy::{
    dyn_abi::TypedData,
    primitives::{Address, B256, Bytes, Signature},
};
use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
};
use common::{
    amount::Amount,
    jobs::Job,
    status::OrderStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AppState, normalize_order_id, now};
use crate::error::ApiError;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectRequest {
    quote_id: String,
    /// 65 bytes (r, s, v) as 0x hex, from the wallet's `signTypedData` on the quote's note.
    signature: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectAccepted {
    status: OrderStatus,
    job_id: i64,
}

#[derive(sqlx::FromRow)]
struct QuotedOrder {
    payer: String,
    typed_data: Value,
    nonce: String,
    valid_after: i64,
    valid_before: i64,
    expires_at: i64,
    status: String,
    amount: String,
    merchant_id: i64,
    payout_address: String,
}

pub async fn direct(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<DirectRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<DirectAccepted>), ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let order_id = normalize_order_id(&id).ok_or_else(|| ApiError::not_found("order"))?;

    let quote: QuotedOrder = sqlx::query_as(
        "SELECT q.payer, q.typed_data, q.nonce, q.valid_after, q.valid_before, q.expires_at,
                o.status, o.amount::text AS amount, o.merchant_id, m.payout_address
         FROM quotes q
         JOIN orders o ON o.id = q.order_id
         JOIN merchants m ON m.id = o.merchant_id
         WHERE q.id = $1 AND q.order_id = $2 AND q.path = 'direct'",
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

    let payer: Address = quote.payer.parse().map_err(ApiError::internal)?;
    let signature = recover_check(&body.signature, &quote.typed_data, payer)?;

    let job = Job::DirectPay {
        order_id: order_id.parse::<B256>().map_err(ApiError::internal)?,
        merchant: quote.payout_address.parse().map_err(ApiError::internal)?,
        amount: quote.amount.parse::<Amount>().map_err(ApiError::internal)?,
        payer,
        valid_after: u64::try_from(quote.valid_after).map_err(ApiError::internal)?,
        valid_before: u64::try_from(quote.valid_before).map_err(ApiError::internal)?,
        nonce: quote.nonce.parse().map_err(ApiError::internal)?,
        signature,
    };
    let chain_id = i64::try_from(state.chains.shop_chain().chain_id).map_err(ApiError::internal)?;

    let mut tx = state.pool.begin().await?;
    // Only from `quoted`, checked again here: a double click cannot make two jobs.
    let moved = sqlx::query("UPDATE orders SET status = 'paying' WHERE id = $1 AND status = 'quoted'")
        .bind(&order_id)
        .execute(&mut *tx)
        .await?;
    if moved.rows_affected() == 0 {
        return Err(ApiError::conflict("already_submitted", "this order's payment is already being sent"));
    }
    let job_id = super::insert_job(&mut tx, &order_id, quote.merchant_id, chain_id, &job, now()).await?;
    tx.commit().await?;

    Ok((StatusCode::ACCEPTED, Json(DirectAccepted { status: OrderStatus::Paying, job_id })))
}

/// The signature must be 65 bytes (an ordinary wallet, per the spec) and recover to the quote's
/// payer from exactly the note we stored. Returns the 65 bytes for the job.
pub(super) fn recover_check(signature: &str, typed_data: &Value, payer: Address) -> Result<Bytes, ApiError> {
    let bad = |message: &str| ApiError::bad_request("bad_signature", message.to_owned());
    let bytes: Bytes = signature.parse().map_err(|_| bad("signature must be 0x hex"))?;
    if bytes.len() != 65 {
        return Err(bad("signature must be 65 bytes"));
    }
    let parsed = Signature::from_raw(&bytes).map_err(|_| bad("signature is not a valid r, s, v"))?;
    let typed: TypedData = serde_json::from_value(typed_data.clone()).map_err(ApiError::internal)?;
    let hash = typed.eip712_signing_hash().map_err(ApiError::internal)?;
    let signer = parsed.recover_address_from_prehash(&hash).map_err(|_| bad("signature does not recover"))?;
    if signer != payer {
        return Err(bad("signature is not from the quote's payer"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
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

    fn post(path: String, body: Value) -> Request<Body> {
        Request::post(path).header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap()
    }

    /// A 12 USDT order quoted for `payer`; returns the order id and the quote reply.
    async fn quoted(test: &TestApp, payer: &PrivateKeySigner) -> (String, Value) {
        let (_, created) = test.create_order(json!({ "amount": "12", "reference": "shop-123" })).await;
        let id = created["orderId"].as_str().unwrap().to_owned();
        let (status, quote) = call(&test.app, post(format!("/v1/orders/{id}/quote"), json!({ "payer": payer.address() }))).await;
        assert_eq!(status, StatusCode::OK, "{quote}");
        (id, quote)
    }

    fn sign(signer: &PrivateKeySigner, quote: &Value) -> String {
        let typed: TypedData = serde_json::from_value(quote["toSign"][0]["typedData"].clone()).unwrap();
        let signature = signer.sign_hash_sync(&typed.eip712_signing_hash().unwrap()).unwrap();
        format!("0x{}", hex::encode(signature.as_bytes()))
    }

    fn submit(id: &str, quote: &Value, signature: &str) -> Request<Body> {
        post(format!("/v1/orders/{id}/direct"), json!({ "quoteId": quote["quoteId"], "signature": signature }))
    }

    #[sqlx::test]
    async fn a_valid_signature_becomes_one_job_and_the_order_is_paying(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(&test, &alice).await;

        let (status, reply) = call(&test.app, submit(&id, &quote, &sign(&alice, &quote))).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{reply}");
        assert_eq!(reply["status"], "paying");

        let (order_status,): (String,) =
            sqlx::query_as("SELECT status FROM orders WHERE id = $1").bind(&id).fetch_one(&test.pool).await.unwrap();
        assert_eq!(order_status, "paying");
        let (kind, job_status, args): (String, String, Value) =
            sqlx::query_as("SELECT kind, status, args FROM relayer_jobs WHERE id = $1")
                .bind(reply["jobId"].as_i64().unwrap())
                .fetch_one(&test.pool)
                .await
                .unwrap();
        assert_eq!((kind.as_str(), job_status.as_str()), ("direct_pay", "pending"));
        let Job::DirectPay { payer, amount, nonce, valid_before, .. } = serde_json::from_value(args).unwrap() else {
            panic!("expected a direct_pay job");
        };
        let message = &quote["toSign"][0]["typedData"]["message"];
        assert_eq!(payer, alice.address());
        assert_eq!(amount.to_string(), "12000000");
        assert_eq!(nonce.to_string(), message["nonce"].as_str().unwrap());
        assert_eq!(valid_before.to_string(), message["validBefore"].as_str().unwrap());
    }

    #[sqlx::test]
    async fn a_signature_from_someone_else_is_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let (alice, mallory) = (PrivateKeySigner::random(), PrivateKeySigner::random());
        let (id, quote) = quoted(&test, &alice).await;
        let (status, reply) = call(&test.app, submit(&id, &quote, &sign(&mallory, &quote))).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_signature")));
    }

    #[sqlx::test]
    async fn malformed_signatures_are_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(&test, &alice).await;
        let good = sign(&alice, &quote);
        for bad in [&good[..good.len() - 2], "0x1234", "not hex", ""] {
            let (status, reply) = call(&test.app, submit(&id, &quote, bad)).await;
            assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_signature")), "{bad}");
        }
    }

    #[sqlx::test]
    async fn submitting_twice_makes_one_job(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(&test, &alice).await;
        let signature = sign(&alice, &quote);
        assert_eq!(call(&test.app, submit(&id, &quote, &signature)).await.0, StatusCode::ACCEPTED);
        let (status, reply) = call(&test.app, submit(&id, &quote, &signature)).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("already_submitted")));
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM relayer_jobs").fetch_one(&test.pool).await.unwrap();
        assert_eq!(jobs, 1);
    }

    #[sqlx::test]
    async fn an_expired_or_unknown_quote_is_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(&test, &alice).await;
        let signature = sign(&alice, &quote);

        let wrong_quote = json!({ "quoteId": "q_nope", "toSign": quote["toSign"] });
        assert_eq!(call(&test.app, submit(&id, &wrong_quote, &signature)).await.0, StatusCode::NOT_FOUND);

        sqlx::query("UPDATE quotes SET expires_at = 1").execute(&test.pool).await.unwrap();
        let (status, reply) = call(&test.app, submit(&id, &quote, &signature)).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("quote_expired")));
    }

    #[sqlx::test]
    async fn no_new_quote_while_a_payment_is_being_sent(pool: PgPool) {
        let test = test_app(pool).await;
        let (alice, bob) = (PrivateKeySigner::random(), PrivateKeySigner::random());
        let (id, quote) = quoted(&test, &alice).await;
        assert_eq!(call(&test.app, submit(&id, &quote, &sign(&alice, &quote))).await.0, StatusCode::ACCEPTED);

        let requote = post(format!("/v1/orders/{id}/quote"), json!({ "payer": bob.address() }));
        let (status, reply) = call(&test.app, requote).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("already_submitted")));
        let (order_status,): (String,) =
            sqlx::query_as("SELECT status FROM orders WHERE id = $1").bind(&id).fetch_one(&test.pool).await.unwrap();
        assert_eq!(order_status, "paying");
    }

    #[sqlx::test]
    async fn a_second_unfinished_job_is_a_conflict_not_a_crash(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(&test, &alice).await;
        let signature = sign(&alice, &quote);
        assert_eq!(call(&test.app, submit(&id, &quote, &signature)).await.0, StatusCode::ACCEPTED);
        // As if the order were reopened while its job is still live.
        sqlx::query("UPDATE orders SET status = 'quoted'").execute(&test.pool).await.unwrap();
        let (status, reply) = call(&test.app, submit(&id, &quote, &signature)).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("already_submitted")));
    }

    #[sqlx::test]
    async fn the_order_shows_its_payment_as_the_relayer_works(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(&test, &alice).await;
        let read = || Request::get(format!("/v1/orders/{id}")).body(Body::empty()).unwrap();
        assert_eq!(call(&test.app, read()).await.1["payment"], Value::Null);

        call(&test.app, submit(&id, &quote, &sign(&alice, &quote))).await;
        let payment = call(&test.app, read()).await.1["payment"].clone();
        assert_eq!(payment, json!({ "status": "pending", "txHash": null, "explorerUrl": null, "failedReason": null }));

        let hash = format!("0x{}", "ab".repeat(32));
        sqlx::query("UPDATE relayer_jobs SET status = 'sent', tx_hash = $1, raw_tx = '\\x02'")
            .bind(&hash)
            .execute(&test.pool)
            .await
            .unwrap();
        let payment = call(&test.app, read()).await.1["payment"].clone();
        assert_eq!(payment["txHash"], hash.as_str());
        assert_eq!(payment["explorerUrl"], format!("https://explorer.testnet.whitechain.io/tx/{hash}"));

        sqlx::query("UPDATE relayer_jobs SET status = 'failed', last_error = 'the payer does not have enough USDT'")
            .execute(&test.pool)
            .await
            .unwrap();
        let payment = call(&test.app, read()).await.1["payment"].clone();
        assert_eq!(payment["failedReason"], "the payer does not have enough USDT");
    }
}
