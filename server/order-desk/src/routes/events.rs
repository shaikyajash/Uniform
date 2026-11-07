//! `POST /internal/events`: the Matchmaker reports one chain event it has seen and confirmed.
//!
//! Every event is stored once (`events`, keyed by chain, tx and log index). Money facts come only
//! from here: `Paid` (Path A) is applied below; the cross-chain kinds in `crosschain_events.rs`.
//! `DirectRefunded` is stored only: shop refunds go through our Relayer, which moves the order when
//! the refund is mined (relayer `apply_to_order`).

use alloy::primitives::Address;
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, header::AUTHORIZATION},
};
use common::{
    amount::Amount,
    events::{ChainEvent, EventKind},
    status::OrderStatus,
};
use serde::Serialize;
use sqlx::{Postgres, Transaction};

use super::{
    AppState,
    crosschain_events::{self, Applied},
    now,
};
use crate::{direct_pay, error::ApiError};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventReceipt {
    /// False when this event was already reported: nothing changed.
    stored: bool,
    /// Whether the event moved an order.
    applied: bool,
    order_status: Option<OrderStatus>,
    /// Why an event was stored but not applied.
    reason: Option<String>,
}

impl EventReceipt {
    fn duplicate() -> Self {
        EventReceipt { stored: false, applied: false, order_status: None, reason: Some("already reported".into()) }
    }

    fn not_applied(reason: impl Into<String>) -> Self {
        EventReceipt { stored: true, applied: false, order_status: None, reason: Some(reason.into()) }
    }

    fn moved_to(status: OrderStatus) -> Self {
        EventReceipt { stored: true, applied: true, order_status: Some(status), reason: None }
    }
}

pub async fn report(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<ChainEvent>, JsonRejection>,
) -> Result<Json<EventReceipt>, ApiError> {
    if !has_token(&headers, &state.internal_token) {
        return Err(ApiError::unauthorized());
    }
    let Json(event) = body.map_err(|rejection| ApiError::bad_request("bad_event", rejection.body_text()))?;

    let mut tx = state.pool.begin().await?;
    let stored = sqlx::query(
        "INSERT INTO events (chain_id, tx_hash, log_index, block_number, name, order_id, data, received_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (chain_id, tx_hash, log_index) DO NOTHING",
    )
    .bind(i64::try_from(event.chain_id).map_err(|_| ApiError::bad_request("bad_event", "chainId too large"))?)
    .bind(event.tx_hash.to_string())
    .bind(i64::try_from(event.log_index).map_err(|_| ApiError::bad_request("bad_event", "logIndex too large"))?)
    .bind(i64::try_from(event.block_number).map_err(|_| ApiError::bad_request("bad_event", "blockNumber too large"))?)
    .bind(event.kind.name())
    .bind(order_id_of(&event.kind))
    .bind(serde_json::to_value(&event).map_err(ApiError::internal)?)
    .bind(now())
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !stored {
        return Ok(Json(EventReceipt::duplicate()));
    }

    let receipt = match &event.kind {
        EventKind::Paid { payment_id, order_id, payer, amount } => {
            let paid = PaidEvent { chain_id: event.chain_id, payment_id, order_id, payer: *payer, amount: *amount };
            apply_paid(&mut tx, &state, paid).await?
        }
        EventKind::Locked { lock_id, token, amount, .. } => {
            let applied = crosschain_events::locked(&mut tx, event.chain_id, event.tx_hash, *lock_id, *token, *amount);
            receipt(applied.await?)
        }
        EventKind::Filled { order_id, destination_lock_id, amount } => {
            receipt(crosschain_events::filled(&mut tx, &state, *order_id, *destination_lock_id, *amount).await?)
        }
        EventKind::Settled { order_id, amount } => receipt(crosschain_events::settled(&mut tx, *order_id, *amount).await?),
        EventKind::Claimed { lock_id } => {
            receipt(crosschain_events::claimed(&mut tx, event.chain_id, event.tx_hash, *lock_id).await?)
        }
        EventKind::Refunded { lock_id } => {
            receipt(crosschain_events::refunded(&mut tx, &state, event.chain_id, event.tx_hash, *lock_id).await?)
        }
        other => EventReceipt::not_applied(format!("{} events are applied in a later task", other.name())),
    };
    if !receipt.applied && receipt.reason.is_some() {
        eprintln!("order-desk: stored {} event {} but did not apply it: {:?}", event.kind.name(), event.tx_hash, receipt.reason);
    }
    tx.commit().await?;
    Ok(Json(receipt))
}

fn receipt(applied: Applied) -> EventReceipt {
    match applied {
        Applied::Moved(status) => EventReceipt::moved_to(status),
        Applied::LegOnly(what) => {
            EventReceipt { stored: true, applied: true, order_status: None, reason: Some(what.into()) }
        }
        Applied::Skipped(reason) => EventReceipt::not_applied(reason),
    }
}

struct PaidEvent<'a> {
    chain_id: u64,
    payment_id: &'a alloy::primitives::B256,
    order_id: &'a alloy::primitives::B256,
    payer: Address,
    amount: Amount,
}

/// DirectPay's `Paid`: the shop has the USDT. Moves the order to `paid` only if the event is for
/// exactly this order's shop and amount (SPEC: check merchant and amount against the order).
async fn apply_paid(
    tx: &mut Transaction<'_, Postgres>,
    state: &AppState,
    paid: PaidEvent<'_>,
) -> Result<EventReceipt, ApiError> {
    let chain = state.chains.shop_chain();
    if paid.chain_id != chain.chain_id {
        return Ok(EventReceipt::not_applied(format!("Paid on chain {}, not the merchant's chain", paid.chain_id)));
    }
    let direct_pay = chain.contracts.direct_pay.ok_or_else(|| ApiError::internal("chains.json has no DirectPay"))?;

    let order: Option<(String, String, String)> = sqlx::query_as(
        "SELECT o.amount::text, o.status, m.payout_address
         FROM orders o JOIN merchants m ON m.id = o.merchant_id
         WHERE o.id = $1
         FOR UPDATE OF o",
    )
    .bind(paid.order_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    let Some((amount, status, payout_address)) = order else {
        return Ok(EventReceipt::not_applied("no such order"));
    };
    let amount: Amount = amount.parse().map_err(ApiError::internal)?;
    let status: OrderStatus = status.parse().map_err(ApiError::internal)?;
    let merchant: Address = payout_address.parse().map_err(ApiError::internal)?;

    if paid.amount != amount {
        return Ok(EventReceipt::not_applied(format!("paid {} but the order is {}", paid.amount, amount)));
    }
    // paymentId commits to the merchant, so a match proves the USDT went to this order's shop.
    let expected = direct_pay::payment_id_for(chain.chain_id, direct_pay, *paid.order_id, merchant, amount, paid.payer);
    if *paid.payment_id != expected {
        return Ok(EventReceipt::not_applied("paymentId does not match this order's merchant, amount and payer"));
    }

    // The relayer sets `paying` when it sends (B2). A confirmed payment can arrive first, so walk
    // through `paying`; every step must still be an allowed move.
    let steps: &[OrderStatus] = match status {
        OrderStatus::Quoted => &[OrderStatus::Paying, OrderStatus::Paid],
        OrderStatus::Paying => &[OrderStatus::Paid],
        OrderStatus::Paid => return Ok(EventReceipt::not_applied("order is already paid")),
        other => return Ok(EventReceipt::not_applied(format!("order is {other}; a payment cannot move it"))),
    };
    let mut from = status;
    for &to in steps {
        if !from.can_move(to) {
            return Err(ApiError::internal(format!("status table does not allow {from} -> {to}")));
        }
        from = to;
    }

    sqlx::query("UPDATE orders SET status = 'paid', payer = $2, paid_at = $3 WHERE id = $1")
        .bind(paid.order_id.to_string())
        .bind(format!("{:#x}", paid.payer))
        .bind(now())
        .execute(&mut **tx)
        .await?;
    Ok(EventReceipt::moved_to(OrderStatus::Paid))
}

fn order_id_of(kind: &EventKind) -> Option<String> {
    match kind {
        EventKind::Paid { order_id, .. }
        | EventKind::DirectRefunded { order_id, .. }
        | EventKind::Locked { order_id, .. }
        | EventKind::Filled { order_id, .. }
        | EventKind::Settled { order_id, .. } => Some(order_id.to_string()),
        EventKind::Claimed { .. } | EventKind::Refunded { .. } => None,
    }
}

/// `Authorization: Bearer <INTERNAL_EVENTS_TOKEN>`, compared in constant time.
pub(crate) fn has_token(headers: &HeaderMap, expected: &str) -> bool {
    let Some(given) = headers.get(AUTHORIZATION).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Some(given) = given.strip_prefix("Bearer ") else {
        return false;
    };
    !expected.is_empty()
        && given.len() == expected.len()
        && given.bytes().zip(expected.bytes()).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{Address, B256, address};
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use common::{
        amount::Amount,
        events::{ChainEvent, EventKind},
    };
    use serde_json::{Value, json};
    use sqlx::PgPool;

    use crate::{
        direct_pay::payment_id_for,
        routes::test_support::{INTERNAL_TOKEN, TestApp, call, test_app},
    };

    const DIRECT_PAY: Address = address!("9DF1C7C32Ce3f1322e1c7bAEc30664b1c472DaE9");
    const SHOP: Address = address!("5a30000000000000000000000000000000000001");
    const PAYER: Address = address!("a917000000000000000000000000000000000001");

    fn report(token: Option<&str>, body: &Value) -> Request<Body> {
        let mut request = Request::post("/internal/events").header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        request.body(Body::from(body.to_string())).unwrap()
    }

    /// An order for 12 USDT, quoted for PAYER: the state a Path A payment starts from.
    async fn quoted_order(test: &TestApp) -> B256 {
        let (_, created) = test.create_order(json!({ "amount": "12", "reference": "shop-123" })).await;
        let id = created["orderId"].as_str().unwrap().to_owned();
        let quote = Request::post(format!("/v1/orders/{id}/quote"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "payer": PAYER }).to_string()))
            .unwrap();
        assert_eq!(call(&test.app, quote).await.0, StatusCode::OK);
        id.parse().unwrap()
    }

    fn paid_event(order_id: B256, merchant: Address, amount: u64) -> Value {
        let amount = Amount::from(amount);
        let event = ChainEvent {
            chain_id: 1874,
            block_number: 5_000_000,
            tx_hash: B256::repeat_byte(0x77),
            log_index: 3,
            kind: EventKind::Paid {
                payment_id: payment_id_for(1874, DIRECT_PAY, order_id, merchant, amount, PAYER),
                order_id,
                payer: PAYER,
                amount,
            },
        };
        serde_json::to_value(event).unwrap()
    }

    async fn order_status(test: &TestApp, order_id: B256) -> (String, Option<i64>) {
        sqlx::query_as("SELECT status, paid_at FROM orders WHERE id = $1")
            .bind(order_id.to_string())
            .fetch_one(&test.pool)
            .await
            .unwrap()
    }

    #[sqlx::test]
    async fn a_valid_paid_event_marks_the_order_paid_once(pool: PgPool) {
        let test = test_app(pool).await;
        let order_id = quoted_order(&test).await;
        let event = paid_event(order_id, SHOP, 12_000_000);

        let (status, receipt) = call(&test.app, report(Some(INTERNAL_TOKEN), &event)).await;
        assert_eq!(status, StatusCode::OK, "{receipt}");
        assert_eq!(receipt, json!({ "stored": true, "applied": true, "orderStatus": "paid", "reason": null }));
        let (order, paid_at) = order_status(&test, order_id).await;
        assert_eq!(order, "paid");
        assert!(paid_at.is_some());

        let (_, again) = call(&test.app, report(Some(INTERNAL_TOKEN), &event)).await;
        assert_eq!((again["stored"].as_bool(), again["applied"].as_bool()), (Some(false), Some(false)));
        let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM events").fetch_one(&test.pool).await.unwrap();
        assert_eq!(stored, 1);
    }

    #[sqlx::test]
    async fn a_payment_for_the_wrong_amount_or_shop_is_stored_but_not_applied(pool: PgPool) {
        let test = test_app(pool).await;
        let order_id = quoted_order(&test).await;

        let short = paid_event(order_id, SHOP, 11_000_000);
        let (_, receipt) = call(&test.app, report(Some(INTERNAL_TOKEN), &short)).await;
        assert_eq!((receipt["stored"].as_bool(), receipt["applied"].as_bool()), (Some(true), Some(false)), "{receipt}");

        let mut other_shop = paid_event(order_id, address!("0000000000000000000000000000000000000bad"), 12_000_000);
        other_shop["logIndex"] = json!(4);
        let (_, receipt) = call(&test.app, report(Some(INTERNAL_TOKEN), &other_shop)).await;
        assert_eq!(receipt["applied"], false, "{receipt}");
        assert!(receipt["reason"].as_str().unwrap().contains("paymentId"));

        assert_eq!(order_status(&test, order_id).await.0, "quoted");
    }

    #[sqlx::test]
    async fn a_lock_we_did_not_plan_is_stored_but_moves_nothing(pool: PgPool) {
        let test = test_app(pool).await;
        let locked = json!({
            "chainId": 84532, "blockNumber": 1, "txHash": format!("0x{}", "11".repeat(32)), "logIndex": 0,
            "type": "Locked", "lockId": format!("0x{}", "22".repeat(32)),
            "orderId": format!("0x{}", "33".repeat(32)), "token": format!("0x{}", "44".repeat(20)), "amount": "12036000",
        });
        let (status, receipt) = call(&test.app, report(Some(INTERNAL_TOKEN), &locked)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!((receipt["stored"].as_bool(), receipt["applied"].as_bool()), (Some(true), Some(false)));
    }

    #[sqlx::test]
    async fn only_the_matchmaker_token_may_report(pool: PgPool) {
        let test = test_app(pool).await;
        let order_id = quoted_order(&test).await;
        let event = paid_event(order_id, SHOP, 12_000_000);
        for token in [None, Some("wrong-token-0123456789abcdefghi"), Some(&INTERNAL_TOKEN[..10])] {
            let (status, _) = call(&test.app, report(token, &event)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{token:?}");
        }
        assert_eq!(order_status(&test, order_id).await.0, "quoted");
    }

    #[sqlx::test]
    async fn malformed_events_get_400(pool: PgPool) {
        let test = test_app(pool).await;
        let (status, reply) = call(&test.app, report(Some(INTERNAL_TOKEN), &json!({ "type": "Teleported" }))).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_event")));
    }
}
