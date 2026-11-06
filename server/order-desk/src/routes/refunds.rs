//! Shop refunds of Path A payments, signed by the shop's payout wallet in the dashboard.
//!
//! 1. `POST /v1/orders/{id}/refund-note {amount}`: the note to sign. It is a USDT
//!    `ReceiveWithAuthorization` from the shop to DirectPay; its nonce is
//!    `refundNonceFor(paymentId, amount, refundedBefore)`, so it works for this refund only.
//! 2. The owner signs it in their wallet.
//! 3. `POST /v1/orders/{id}/refunds {amount, validAfter, validBefore, signature}`: the Order Desk
//!    rebuilds the same note, checks the signature is the payout wallet's, and writes a
//!    `direct_refund` job. The Relayer sends `DirectPay.refund`, which returns the USDT to the
//!    customer who paid. Once mined the order is `partially_refunded` or `refunded`.
//!
//! Both steps need a dashboard session. Cross-chain payments cannot be refunded yet (needs A9).

use alloy::primitives::{Address, B256};
use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
};
use common::{amount::Amount, jobs::Job};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AppState, direct::recover_check, normalize_order_id, now};
use crate::{
    auth::Owner,
    direct_pay::{self, Note, payment_id_for, refund_nonce_for},
    error::ApiError,
};

const USDT_DECIMALS: u8 = 6;
/// The note lives this long. DirectPay refuses one valid for more than an hour.
const NOTE_SECONDS: i64 = 30 * 60;
/// Same slack as customer notes: `validAfter` a little in the past.
const CLOCK_SLACK_SECONDS: i64 = 60;
/// DirectPay's `MAX_AUTH_WINDOW`.
const MAX_WINDOW_SECONDS: i64 = 3600;

/// What a refund of this order is built from, checked against the shop.
struct Refundable {
    order_id: B256,
    payment_id: B256,
    merchant: Address,
    amount: Amount,
    refunded: Amount,
}

#[derive(sqlx::FromRow)]
struct Basis {
    amount: String,
    status: String,
    path: Option<String>,
    payer: Option<String>,
    payout_address: String,
    refunded: String,
    live: bool,
}

async fn refundable(state: &AppState, merchant_id: i64, id: &str) -> Result<Refundable, ApiError> {
    let order_id = normalize_order_id(id).ok_or_else(|| ApiError::not_found("order"))?;
    let basis: Basis = sqlx::query_as(
        "SELECT o.amount::text AS amount, o.status, o.path, o.payer, m.payout_address,
                COALESCE((SELECT SUM((j.args->>'amount')::numeric) FROM relayer_jobs j
                          WHERE j.order_id = o.id AND j.kind = 'direct_refund' AND j.status = 'confirmed'), 0)::text
                    AS refunded,
                EXISTS (SELECT 1 FROM relayer_jobs j WHERE j.order_id = o.id AND j.kind = 'direct_refund'
                        AND j.status IN ('pending', 'sending', 'sent')) AS live
         FROM orders o JOIN merchants m ON m.id = o.merchant_id
         WHERE o.id = $1 AND o.merchant_id = $2",
    )
    .bind(&order_id)
    .bind(merchant_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("order"))?;

    // Only a Path A payment went through DirectPay, which is what refunds it. Cross-chain and split
    // payments need A9.
    if basis.path.as_deref() != Some("direct") {
        return Err(ApiError::conflict("refund_unavailable", "only USDT payments can be refunded from here yet"));
    }
    if !matches!(basis.status.as_str(), "paid" | "partially_refunded") {
        return Err(ApiError::conflict("not_refundable", format!("the order is {}", basis.status)));
    }
    if basis.live {
        return Err(ApiError::conflict("refund_in_progress", "a refund for this order is already being sent"));
    }
    let payer: Address = basis.payer.as_deref().ok_or_else(|| ApiError::internal("paid order without a payer"))?.parse().map_err(ApiError::internal)?;
    let merchant: Address = basis.payout_address.parse().map_err(ApiError::internal)?;
    let amount: Amount = basis.amount.parse().map_err(ApiError::internal)?;
    let order_id: B256 = order_id.parse().map_err(ApiError::internal)?;
    let chain = state.chains.shop_chain();
    let direct_pay = chain.contracts.direct_pay.ok_or_else(|| ApiError::internal("chains.json has no DirectPay"))?;
    Ok(Refundable {
        order_id,
        payment_id: payment_id_for(chain.chain_id, direct_pay, order_id, merchant, amount, payer),
        merchant,
        amount,
        refunded: basis.refunded.parse().map_err(ApiError::internal)?,
    })
}

/// The refund amount from decimal text, at most what is left to refund.
fn refund_amount(text: &str, order: &Refundable) -> Result<Amount, ApiError> {
    let amount = Amount::from_decimal(text, USDT_DECIMALS)
        .map_err(|_| ApiError::bad_request("bad_amount", "amount must be USDT as decimal text, like \"1.50\""))?;
    let left = Amount(order.amount.0.saturating_sub(order.refunded.0));
    if amount == Amount::default() || amount > left {
        let left = left.to_decimal(USDT_DECIMALS);
        return Err(ApiError::bad_request("bad_amount", format!("amount must be above zero and at most {left} USDT")));
    }
    Ok(amount)
}

/// The note, exactly as signed and as checked.
fn note_json(state: &AppState, order: &Refundable, amount: Amount, valid_after: i64, valid_before: i64) -> Result<(Value, B256), ApiError> {
    let chain = state.chains.shop_chain();
    let direct_pay = chain.contracts.direct_pay.ok_or_else(|| ApiError::internal("chains.json has no DirectPay"))?;
    let token = chain.tokens.get("USDT").ok_or_else(|| ApiError::internal("chains.json has no USDT"))?;
    let nonce = refund_nonce_for(chain.chain_id, direct_pay, order.payment_id, amount, order.refunded);
    let note = Note { from: order.merchant, to: direct_pay, value: amount, valid_after, valid_before, nonce };
    Ok((direct_pay::typed_data(token, chain.chain_id, &note), nonce))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NoteRequest {
    amount: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefundNote {
    amount: String,
    valid_after: i64,
    valid_before: i64,
    typed_data: Value,
}

pub async fn note(
    State(state): State<AppState>,
    owner: Owner,
    Path(id): Path<String>,
    body: Result<Json<NoteRequest>, JsonRejection>,
) -> Result<Json<RefundNote>, ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let order = refundable(&state, owner.merchant_id, &id).await?;
    let amount = refund_amount(&body.amount, &order)?;
    let valid_after = now() - CLOCK_SLACK_SECONDS;
    let valid_before = now() + NOTE_SECONDS;
    let (typed_data, _) = note_json(&state, &order, amount, valid_after, valid_before)?;
    Ok(Json(RefundNote { amount: amount.to_decimal(USDT_DECIMALS), valid_after, valid_before, typed_data }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefundRequest {
    amount: String,
    valid_after: i64,
    valid_before: i64,
    /// 65 bytes as 0x hex, the payout wallet's `signTypedData` on the note.
    signature: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefundAccepted {
    job_id: i64,
}

pub async fn submit(
    State(state): State<AppState>,
    owner: Owner,
    Path(id): Path<String>,
    body: Result<Json<RefundRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<RefundAccepted>), ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let order = refundable(&state, owner.merchant_id, &id).await?;
    let amount = refund_amount(&body.amount, &order)?;
    let now = now();
    if body.valid_before <= now + CLOCK_SLACK_SECONDS || body.valid_before > now + MAX_WINDOW_SECONDS || body.valid_after >= now {
        return Err(ApiError::conflict("note_expired", "the refund note ran out; sign a new one"));
    }
    // Rebuilt from the database, not taken from the client: if another refund landed in between,
    // the nonce differs and the signature does not match.
    let (typed_data, nonce) = note_json(&state, &order, amount, body.valid_after, body.valid_before)?;
    let signature = recover_check(&body.signature, &typed_data, order.merchant)
        .map_err(|_| ApiError::bad_request("bad_signature", "the refund must be signed by the shop's payout wallet; sign again"))?;

    let job = Job::DirectRefund {
        order_id: order.order_id,
        payment_id: order.payment_id,
        merchant: order.merchant,
        amount,
        refunded_before: order.refunded,
        valid_after: u64::try_from(body.valid_after).map_err(ApiError::internal)?,
        valid_before: u64::try_from(body.valid_before).map_err(ApiError::internal)?,
        nonce,
        signature,
    };
    let chain_id = i64::try_from(state.chains.shop_chain().chain_id).map_err(ApiError::internal)?;
    let mut tx = state.pool.begin().await?;
    let job_id = super::insert_job(&mut tx, &order.order_id.to_string(), owner.merchant_id, chain_id, &job, now)
        .await
        .map_err(|error| match error.code {
            "already_submitted" => ApiError::conflict("refund_in_progress", "a refund for this order is already being sent"),
            _ => error,
        })?;
    tx.commit().await?;
    Ok((StatusCode::ACCEPTED, Json(RefundAccepted { job_id })))
}

#[cfg(test)]
mod tests {
    use alloy::{
        dyn_abi::TypedData,
        signers::{SignerSync, local::PrivateKeySigner},
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use serde_json::{Value, json};
    use sqlx::PgPool;

    use crate::{
        auth::new_session_token,
        routes::test_support::{TestApp, call, test_app},
    };

    /// A paid 12 USDT Path A order, a shop whose payout wallet we hold, and a session.
    async fn paid_order(app: &TestApp) -> (String, PrivateKeySigner, String) {
        let wallet = PrivateKeySigner::random();
        sqlx::query("UPDATE merchants SET payout_address = $1").bind(format!("{:#x}", wallet.address())).execute(&app.pool).await.unwrap();
        let (_, order) = app.create_order(json!({ "amount": "12", "reference": "r-1" })).await;
        let id = order["orderId"].as_str().unwrap().to_owned();
        sqlx::query("UPDATE orders SET status = 'paid', path = 'direct', payer = '0xa917000000000000000000000000000000000001'")
            .execute(&app.pool)
            .await
            .unwrap();
        let (token, hash) = new_session_token();
        sqlx::query("INSERT INTO sessions (token_hash, merchant_id, created_at, expires_at) SELECT $1, id, 1, $2 FROM merchants")
            .bind(hash)
            .bind(i64::MAX)
            .execute(&app.pool)
            .await
            .unwrap();
        (id, wallet, token)
    }

    async fn post(app: &TestApp, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::post(path)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body.to_string()))
            .unwrap();
        call(&app.app, request).await
    }

    fn sign(wallet: &PrivateKeySigner, typed_data: &Value) -> String {
        let typed: TypedData = serde_json::from_value(typed_data.clone()).unwrap();
        wallet.sign_hash_sync(&typed.eip712_signing_hash().unwrap()).unwrap().to_string()
    }

    #[sqlx::test]
    async fn the_payout_wallet_signs_a_refund_and_a_job_is_written(pool: PgPool) {
        let app = test_app(pool).await;
        let (id, wallet, token) = paid_order(&app).await;
        let (status, note) = post(&app, &format!("/v1/orders/{id}/refund-note"), &token, json!({ "amount": "5" })).await;
        assert_eq!(status, StatusCode::OK, "{note}");
        assert_eq!(note["typedData"]["message"]["from"], format!("{:#x}", wallet.address()));
        assert_eq!(note["typedData"]["message"]["value"], "5000000");

        let body = json!({
            "amount": "5",
            "validAfter": note["validAfter"],
            "validBefore": note["validBefore"],
            "signature": sign(&wallet, &note["typedData"]),
        });
        let (status, reply) = post(&app, &format!("/v1/orders/{id}/refunds"), &token, body.clone()).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{reply}");
        let args: Value = sqlx::query_scalar("SELECT args FROM relayer_jobs WHERE kind = 'direct_refund'").fetch_one(&app.pool).await.unwrap();
        assert_eq!((args["amount"].as_str(), args["refundedBefore"].as_str()), (Some("5000000"), Some("0")));

        let (status, reply) = post(&app, &format!("/v1/orders/{id}/refunds"), &token, body).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("refund_in_progress")));
    }

    #[sqlx::test]
    async fn another_wallet_or_too_much_is_refused(pool: PgPool) {
        let app = test_app(pool).await;
        let (id, _, token) = paid_order(&app).await;
        let (status, _) = post(&app, &format!("/v1/orders/{id}/refund-note"), &token, json!({ "amount": "12.000001" })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (_, note) = post(&app, &format!("/v1/orders/{id}/refund-note"), &token, json!({ "amount": "12" })).await;
        let body = json!({
            "amount": "12",
            "validAfter": note["validAfter"],
            "validBefore": note["validBefore"],
            "signature": sign(&PrivateKeySigner::random(), &note["typedData"]),
        });
        let (status, reply) = post(&app, &format!("/v1/orders/{id}/refunds"), &token, body).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_signature")));
    }

    #[sqlx::test]
    async fn only_a_usdt_payment_can_be_refunded(pool: PgPool) {
        let app = test_app(pool).await;
        let (id, _, token) = paid_order(&app).await;
        for path in ["crosschain", "split"] {
            sqlx::query("UPDATE orders SET path = $1").bind(path).execute(&app.pool).await.unwrap();
            let (status, reply) = post(&app, &format!("/v1/orders/{id}/refund-note"), &token, json!({ "amount": "1" })).await;
            assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("refund_unavailable")), "{path}");
        }
    }

    #[sqlx::test]
    async fn the_api_key_cannot_refund(pool: PgPool) {
        let app = test_app(pool).await;
        let (id, _, _) = paid_order(&app).await;
        let key = app.key.clone();
        let (status, _) = post(&app, &format!("/v1/orders/{id}/refund-note"), &key, json!({ "amount": "1" })).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
