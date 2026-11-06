//! `POST /v1/orders/{id}/quote`: the pay page asks what the customer must sign.
//!
//! B1 handles Path A only (USDT on Whitechain through DirectPay). Paths B and C come in B4.

use alloy::{
    hex,
    primitives::{Address, B256},
};
use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
};
use common::{
    amount::Amount,
    status::{OrderStatus, PayPath},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AppState, normalize_order_id, now};
use crate::{
    direct_pay::{self, Note},
    error::ApiError,
    random,
};

const USDT: &str = "USDT";
/// `validAfter` = now - 60, so a customer whose clock runs a little behind can still sign (SPEC).
const CLOCK_SLACK_SECONDS: i64 = 60;
/// `validBefore` = now + 600 (SPEC). DirectPay rejects anything more than 1 hour ahead.
const NOTE_LIFETIME_SECONDS: i64 = 600;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRequest {
    /// The customer's wallet. The server reads balances itself instead of trusting the browser.
    payer: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteView {
    quote_id: String,
    path: PayPath,
    expires_at: i64,
    /// What the customer pays in total, and Bob's part of it (zero on Path A).
    total: String,
    fee: String,
    /// One note per chain the customer pays from. Path A: exactly one.
    to_sign: Vec<ToSign>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToSign {
    chain_id: u64,
    token: &'static str,
    amount: String,
    typed_data: Value,
}

#[derive(sqlx::FromRow)]
struct PayableOrder {
    amount: String,
    status: String,
    expires_at: i64,
    payout_address: String,
}

pub async fn quote(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<QuoteRequest>, JsonRejection>,
) -> Result<Json<QuoteView>, ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let payer: Address =
        body.payer.parse().map_err(|_| ApiError::bad_request("bad_payer", "payer must be a 0x wallet address"))?;
    let order_id = normalize_order_id(&id).ok_or_else(|| ApiError::not_found("order"))?;

    let order: PayableOrder = sqlx::query_as(
        "SELECT o.amount::text AS amount, o.status, o.expires_at, m.payout_address
         FROM orders o JOIN merchants m ON m.id = o.merchant_id
         WHERE o.id = $1",
    )
    .bind(&order_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("order"))?;

    let status: OrderStatus = order.status.parse().map_err(ApiError::internal)?;
    if !status.can_move(OrderStatus::Quoted) {
        return Err(ApiError::conflict("order_not_payable", format!("order is {status}")));
    }
    let now = now();
    if now >= order.expires_at {
        return Err(ApiError::conflict("order_expired", "the time to pay this order has run out"));
    }

    let chain = state.chains.shop_chain();
    let usdt = chain.tokens.get(USDT).ok_or_else(|| ApiError::internal("chains.json has no USDT on the shop chain"))?;
    let direct_pay =
        chain.contracts.direct_pay.ok_or_else(|| ApiError::internal("chains.json has no DirectPay address"))?;
    let amount: Amount = order.amount.parse().map_err(ApiError::internal)?;
    let merchant: Address = order.payout_address.parse().map_err(ApiError::internal)?;
    let order_bytes: B256 = order_id.parse().map_err(ApiError::internal)?;

    let note = Note {
        from: payer,
        to: direct_pay,
        value: amount,
        valid_after: now - CLOCK_SLACK_SECONDS,
        valid_before: now + NOTE_LIFETIME_SECONDS,
        nonce: direct_pay::payment_id_for(chain.chain_id, direct_pay, order_bytes, merchant, amount, payer),
    };
    let typed_data = direct_pay::typed_data(usdt, chain.chain_id, &note);
    let quote_id = format!("q_{}", hex::encode(&random::bytes32()[..16]));
    let timeout = i64::try_from(state.chains.timeouts_seconds.quote).map_err(ApiError::internal)?;
    let expires_at = (now + timeout).min(order.expires_at);

    // Only from a status that may move to `quoted`, checked again inside the update so two quotes
    // racing on one order cannot both pass.
    let from_statuses: Vec<&str> = OrderStatus::ALL
        .iter()
        .filter(|status| status.can_move(OrderStatus::Quoted))
        .map(|status| status.as_str())
        .collect();
    let mut tx = state.pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE orders SET status = 'quoted', path = 'direct', payer = $2
         WHERE id = $1 AND status = ANY($3)",
    )
    .bind(&order_id)
    .bind(format!("{payer:#x}"))
    .bind(&from_statuses)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::conflict("order_not_payable", "the order changed while quoting; try again"));
    }
    sqlx::query(
        "INSERT INTO quotes (id, order_id, path, payer, typed_data, nonce, valid_after, valid_before, expires_at, created_at)
         VALUES ($1, $2, 'direct', $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(&quote_id)
    .bind(&order_id)
    .bind(format!("{payer:#x}"))
    .bind(&typed_data)
    .bind(note.nonce.to_string())
    .bind(note.valid_after)
    .bind(note.valid_before)
    .bind(expires_at)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let total = amount.to_decimal(usdt.decimals);
    Ok(Json(QuoteView {
        quote_id,
        path: PayPath::Direct,
        expires_at,
        fee: Amount::default().to_decimal(usdt.decimals),
        to_sign: vec![ToSign { chain_id: chain.chain_id, token: USDT, amount: total.clone(), typed_data }],
        total,
    }))
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{Address, B256, address};
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use common::amount::Amount;
    use serde_json::{Value, json};
    use sqlx::PgPool;

    use crate::{
        direct_pay::payment_id_for,
        routes::test_support::{TestApp, call, test_app},
    };

    const PAYER: &str = "0xa917000000000000000000000000000000000001";

    fn quote(id: &str, body: Value) -> Request<Body> {
        Request::post(format!("/v1/orders/{id}/quote"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn new_order(test: &TestApp) -> String {
        let (_, created) = test.create_order(json!({ "amount": "12", "reference": "shop-123" })).await;
        created["orderId"].as_str().unwrap().to_owned()
    }

    #[sqlx::test]
    async fn path_a_quote_is_a_note_for_direct_pay(pool: PgPool) {
        let test = test_app(pool).await;
        let id = new_order(&test).await;
        let (status, reply) = call(&test.app, quote(&id, json!({ "payer": PAYER }))).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!(reply["path"], "direct");
        assert_eq!((reply["total"].as_str(), reply["fee"].as_str()), (Some("12.000000"), Some("0.000000")));

        let note = &reply["toSign"][0];
        assert_eq!(reply["toSign"].as_array().unwrap().len(), 1);
        assert_eq!(note["chainId"], 1874);
        let typed = &note["typedData"];
        assert_eq!(
            typed["domain"],
            json!({ "name": "Tether USD", "version": "1", "chainId": 1874,
                    "verifyingContract": "0xcedfb56113143d55b2358b8cfbb0deb5ad2f55a7" })
        );
        assert_eq!(typed["primaryType"], "ReceiveWithAuthorization");
        let message = &typed["message"];
        assert_eq!(message["from"], PAYER);
        assert_eq!(message["to"], "0x9df1c7c32ce3f1322e1c7baec30664b1c472dae9");
        assert_eq!(message["value"], "12000000");
        let after: i64 = message["validAfter"].as_str().unwrap().parse().unwrap();
        let before: i64 = message["validBefore"].as_str().unwrap().parse().unwrap();
        assert_eq!(before - after, 660);

        let expected_nonce = payment_id_for(
            1874,
            address!("9DF1C7C32Ce3f1322e1c7bAEc30664b1c472DaE9"),
            id.parse::<B256>().unwrap(),
            address!("5a30000000000000000000000000000000000001"),
            Amount::from(12_000_000u64),
            PAYER.parse::<Address>().unwrap(),
        );
        assert_eq!(message["nonce"], expected_nonce.to_string());

        let (order_status, path, payer): (String, String, String) =
            sqlx::query_as("SELECT status, path, payer FROM orders WHERE id = $1")
                .bind(&id)
                .fetch_one(&test.pool)
                .await
                .unwrap();
        assert_eq!((order_status.as_str(), path.as_str(), payer.as_str()), ("quoted", "direct", PAYER));
        let stored_nonce: String = sqlx::query_scalar("SELECT nonce FROM quotes WHERE id = $1")
            .bind(reply["quoteId"].as_str().unwrap())
            .fetch_one(&test.pool)
            .await
            .unwrap();
        assert_eq!(stored_nonce, expected_nonce.to_string());
    }

    #[sqlx::test]
    async fn quoting_again_replaces_the_quote(pool: PgPool) {
        let test = test_app(pool).await;
        let id = new_order(&test).await;
        let (first, _) = call(&test.app, quote(&id, json!({ "payer": PAYER }))).await;
        let (second, _) = call(&test.app, quote(&id, json!({ "payer": PAYER }))).await;
        assert_eq!((first, second), (StatusCode::OK, StatusCode::OK));
    }

    #[sqlx::test]
    async fn paid_or_expired_orders_cannot_be_quoted(pool: PgPool) {
        let test = test_app(pool).await;
        let paid = new_order(&test).await;
        sqlx::query("UPDATE orders SET status = 'paid' WHERE id = $1").bind(&paid).execute(&test.pool).await.unwrap();
        let (status, reply) = call(&test.app, quote(&paid, json!({ "payer": PAYER }))).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("order_not_payable")));

        let late = new_order(&test).await;
        sqlx::query("UPDATE orders SET expires_at = 1 WHERE id = $1").bind(&late).execute(&test.pool).await.unwrap();
        let (status, reply) = call(&test.app, quote(&late, json!({ "payer": PAYER }))).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("order_expired")));
    }

    #[sqlx::test]
    async fn bad_payer_or_unknown_order(pool: PgPool) {
        let test = test_app(pool).await;
        let id = new_order(&test).await;
        let (status, reply) = call(&test.app, quote(&id, json!({ "payer": "alice" }))).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_payer")));

        let unknown = format!("0x{}", "cd".repeat(32));
        let (status, _) = call(&test.app, quote(&unknown, json!({ "payer": PAYER }))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
