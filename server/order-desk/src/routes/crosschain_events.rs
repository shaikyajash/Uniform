//! How the cross-chain events move legs and orders. `events.rs` stores every event first and
//! calls these inside the same transaction.
//!
//! The rule from the security reviews: anyone can open a lock with any order id, receiver or
//! hashlock, so only an exact lock id we computed ourselves may move anything.
//! - `Locked`: the lock id must be a leg we planned (SourceLock's `lockIdFor` of the signed
//!   quote), on that chain, with the same token and amount.
//! - `Filled`: the destination lock id must be `DestinationLock.lockIdFor(srcChainId, ourLockId,
//!   Bob)`, the one lock only Bob can open for our source lock, and hold at least the order.
//! - `Settled`: only for an order whose verified fill we already have. Known gap: `Settled` carries
//!   no lock id, so a stranger's lock for the same order id could also move it (see the B4 plan).

use alloy::primitives::{Address, B256};
use common::{amount::Amount, locks::destination_lock_id, status::OrderStatus};
use sqlx::PgConnection;

use super::{AppState, now};
use crate::error::ApiError;

/// What an event did.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    /// The order moved to this status.
    Moved(OrderStatus),
    /// A leg changed; the order did not.
    LegOnly(&'static str),
    /// Stored, nothing moved, and why.
    Skipped(String),
}

fn skip(reason: impl Into<String>) -> Result<Applied, ApiError> {
    Ok(Applied::Skipped(reason.into()))
}

#[derive(sqlx::FromRow)]
struct LegRow {
    id: i64,
    order_id: String,
    token: String,
    amount: String,
    status: String,
    quote_id: String,
}

/// SourceLock `Locked` on a customer chain.
pub async fn locked(
    tx: &mut PgConnection,
    chain_id: u64,
    tx_hash: B256,
    lock_id: B256,
    token: Address,
    amount: Amount,
) -> Result<Applied, ApiError> {
    let leg: Option<LegRow> = sqlx::query_as(
        "SELECT id, order_id, token, amount::text AS amount, status, quote_id FROM legs
         WHERE chain_id = $1 AND lock_id = $2 FOR UPDATE",
    )
    .bind(i64::try_from(chain_id).map_err(ApiError::internal)?)
    .bind(lock_id.to_string())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(leg) = leg else { return skip("not a lock we planned") };
    if leg.token != format!("{token:#x}") || leg.amount != amount.to_string() {
        return skip("the lock's token or amount is not what was signed");
    }
    if leg.status != "pending" {
        return skip(format!("leg is already {}", leg.status));
    }
    sqlx::query("UPDATE legs SET status = 'locked', lock_tx = $2 WHERE id = $1")
        .bind(leg.id)
        .bind(tx_hash.to_string())
        .execute(&mut *tx)
        .await?;

    let unlocked: i64 = sqlx::query_scalar("SELECT count(*) FROM legs WHERE quote_id = $1 AND status = 'pending'")
        .bind(&leg.quote_id)
        .fetch_one(&mut *tx)
        .await?;
    if unlocked > 0 {
        return Ok(Applied::LegOnly("leg locked; waiting for the others"));
    }
    let status = order_status(tx, &leg.order_id).await?;
    // The Relayer sets `paying` when it sends; the event can arrive first.
    let steps: &[OrderStatus] = match status {
        OrderStatus::Quoted => &[OrderStatus::Paying, OrderStatus::SourceLocked],
        OrderStatus::Paying => &[OrderStatus::SourceLocked],
        _ => return Ok(Applied::LegOnly("leg locked; the order had already moved on")),
    };
    move_order(tx, &leg.order_id, status, steps).await
}

/// PayoutPool `Filled` on the shop chain: Bob's USDT is locked for the shop.
pub async fn filled(
    tx: &mut PgConnection,
    state: &AppState,
    order_id: B256,
    destination_lock: B256,
    amount: Amount,
) -> Result<Applied, ApiError> {
    let Some(provider) = state.provider else { return skip("no PROVIDER_ADDRESS: cross-chain is off") };
    let shop_chain = state.chains.shop_chain();
    let Some(destination_lock_address) = shop_chain.contracts.destination_lock else {
        return skip("chains.json has no DestinationLock");
    };
    let order_id = order_id.to_string();
    let order: Option<(String, String)> =
        sqlx::query_as("SELECT status, amount::text FROM orders WHERE id = $1 FOR UPDATE")
            .bind(&order_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((status, order_amount)) = order else { return skip("no such order") };
    let status: OrderStatus = status.parse().map_err(ApiError::internal)?;
    let order_amount: Amount = order_amount.parse().map_err(ApiError::internal)?;

    // Our legs, newest first: the fill must be for one of them.
    let legs: Vec<(i64, i64, String, String)> = sqlx::query_as(
        "SELECT id, chain_id, lock_id, status FROM legs WHERE order_id = $1 AND status IN ('pending', 'locked')
         ORDER BY id DESC",
    )
    .bind(&order_id)
    .fetch_all(&mut *tx)
    .await?;
    let ours = legs.iter().find(|(_, chain_id, lock_id, _)| {
        let (Ok(chain_id), Ok(lock_id)) = (u64::try_from(*chain_id), lock_id.parse::<B256>()) else { return false };
        destination_lock_id(shop_chain.chain_id, destination_lock_address, chain_id, lock_id, provider) == destination_lock
    });
    let Some((leg_id, _, _, leg_status)) = ours else {
        return skip("the destination lock is not Bob's lock for one of our source locks");
    };
    if amount.0 < order_amount.0 {
        return skip(format!("filled {amount} but the order is {order_amount}"));
    }
    let steps: &[OrderStatus] = match status {
        OrderStatus::SourceLocked => &[OrderStatus::DestinationLocked],
        // `Locked` may not have been reported yet; Bob fills only after seeing the lock.
        OrderStatus::Paying => &[OrderStatus::SourceLocked, OrderStatus::DestinationLocked],
        OrderStatus::Quoted => &[OrderStatus::Paying, OrderStatus::SourceLocked, OrderStatus::DestinationLocked],
        other => return skip(format!("order is {other}; a fill cannot move it")),
    };
    if leg_status == "pending" {
        sqlx::query("UPDATE legs SET status = 'locked' WHERE id = $1").bind(leg_id).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE orders SET destination_lock_id = $2 WHERE id = $1")
        .bind(&order_id)
        .bind(destination_lock.to_string())
        .execute(&mut *tx)
        .await?;
    move_order(tx, &order_id, status, steps).await
}

/// DestinationLock `Settled` on the shop chain: the shop has the USDT.
pub async fn settled(tx: &mut PgConnection, order_id: B256, amount: Amount) -> Result<Applied, ApiError> {
    let order_id = order_id.to_string();
    let order: Option<(String, String, Option<String>)> =
        sqlx::query_as("SELECT status, amount::text, destination_lock_id FROM orders WHERE id = $1 FOR UPDATE")
            .bind(&order_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((status, order_amount, destination_lock)) = order else { return skip("no such order") };
    let status: OrderStatus = status.parse().map_err(ApiError::internal)?;
    // The Relayer marks the order paid when its own claim is mined; this is the same payment.
    if status == OrderStatus::Paid && destination_lock.is_some() {
        return Ok(Applied::LegOnly("already paid by our claim"));
    }
    if status != OrderStatus::DestinationLocked || destination_lock.is_none() {
        return skip(format!("order is {status}; only a verified fill can be settled"));
    }
    let order_amount: Amount = order_amount.parse().map_err(ApiError::internal)?;
    if amount.0 < order_amount.0 {
        return skip(format!("settled {amount} but the order is {order_amount}"));
    }
    sqlx::query("UPDATE orders SET paid_at = $2 WHERE id = $1").bind(&order_id).bind(now()).execute(&mut *tx).await?;
    move_order(tx, &order_id, status, &[OrderStatus::Paid]).await
}

/// SourceLock `Claimed`: Bob redeemed a customer lock with the public secret.
pub async fn claimed(tx: &mut PgConnection, chain_id: u64, tx_hash: B256, lock_id: B256) -> Result<Applied, ApiError> {
    let updated = sqlx::query(
        "UPDATE legs SET status = 'claimed', claim_tx = $3
         WHERE chain_id = $1 AND lock_id = $2 AND status IN ('pending', 'locked')",
    )
    .bind(i64::try_from(chain_id).map_err(ApiError::internal)?)
    .bind(lock_id.to_string())
    .bind(tx_hash.to_string())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 { skip("not an open lock of ours") } else { Ok(Applied::LegOnly("leg claimed by Bob")) }
}

/// `Refunded` on a customer chain (SourceLock): the customer has their tokens back. On the shop
/// chain it is Bob's own USDT going back to him, which changes nothing here.
pub async fn refunded(
    tx: &mut PgConnection,
    state: &AppState,
    chain_id: u64,
    tx_hash: B256,
    lock_id: B256,
) -> Result<Applied, ApiError> {
    if chain_id == state.chains.shop_chain().chain_id {
        return skip("a destination lock went back to Bob");
    }
    let order_id: Option<String> = sqlx::query_scalar(
        "UPDATE legs SET status = 'returned', refund_tx = $3
         WHERE chain_id = $1 AND lock_id = $2 AND status IN ('pending', 'locked')
         RETURNING order_id",
    )
    .bind(i64::try_from(chain_id).map_err(ApiError::internal)?)
    .bind(lock_id.to_string())
    .bind(tx_hash.to_string())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(order_id) = order_id else { return skip("not an open lock of ours") };
    let status = order_status(tx, &order_id).await?;
    let steps: &[OrderStatus] = match status {
        OrderStatus::SourceLocked | OrderStatus::DestinationLocked => &[OrderStatus::Returned],
        OrderStatus::Paying => &[OrderStatus::SourceLocked, OrderStatus::Returned],
        _ => return Ok(Applied::LegOnly("leg returned; the order had already moved on")),
    };
    move_order(tx, &order_id, status, steps).await
}

async fn order_status(tx: &mut PgConnection, order_id: &str) -> Result<OrderStatus, ApiError> {
    let status: String = sqlx::query_scalar("SELECT status FROM orders WHERE id = $1 FOR UPDATE")
        .bind(order_id)
        .fetch_one(&mut *tx)
        .await?;
    status.parse().map_err(ApiError::internal)
}

/// Walks the order through `steps`, each an allowed move, and saves the last.
async fn move_order(
    tx: &mut PgConnection,
    order_id: &str,
    from: OrderStatus,
    steps: &[OrderStatus],
) -> Result<Applied, ApiError> {
    let mut at = from;
    for &to in steps {
        if !at.can_move(to) {
            return Err(ApiError::internal(format!("status table does not allow {at} -> {to}")));
        }
        at = to;
    }
    sqlx::query("UPDATE orders SET status = $2 WHERE id = $1").bind(order_id).bind(at.as_str()).execute(&mut *tx).await?;
    Ok(Applied::Moved(at))
}

#[cfg(test)]
mod tests {
    use alloy::{primitives::B256, signers::local::PrivateKeySigner};
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use common::locks::destination_lock_id;
    use serde_json::{Value, json};
    use sqlx::PgPool;

    use crate::routes::{
        legs::tests::{quoted, sign, submit},
        test_support::{INTERNAL_TOKEN, PROVIDER, TestApp, call, test_app},
    };

    const DESTINATION_LOCK: &str = "0x669c928135B848C828144360aD7a73391968BDA3";

    /// A 12 USDT cross-chain order whose lock job was handed to the Relayer: (order id, lock id).
    async fn paying(test: &TestApp) -> (String, String) {
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(test, &alice).await;
        assert_eq!(call(&test.app, submit(&id, &quote, &sign(&alice, &quote))).await.0, StatusCode::ACCEPTED);
        (id, quote["toSign"][0]["lockId"].as_str().unwrap().to_owned())
    }

    /// Reports one event; each gets its own tx hash so none is a duplicate.
    async fn report(test: &TestApp, chain_id: u64, kind: Value) -> Value {
        let mut event = json!({ "chainId": chain_id, "blockNumber": 1, "txHash": B256::from(crate::random::bytes32()), "logIndex": 0 });
        event.as_object_mut().unwrap().extend(kind.as_object().unwrap().clone());
        let request = Request::post("/internal/events")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {INTERNAL_TOKEN}"))
            .body(Body::from(event.to_string()))
            .unwrap();
        let (status, receipt) = call(&test.app, request).await;
        assert_eq!(status, StatusCode::OK, "{receipt}");
        receipt
    }

    fn locked(order: &str, lock_id: &str, amount: &str) -> Value {
        json!({ "type": "Locked", "lockId": lock_id, "orderId": order,
                "token": "0x036cbd53842c5426634e7929541ec2318f3dcf7e", "amount": amount })
    }

    fn bobs_lock(lock_id: &str) -> B256 {
        destination_lock_id(1874, DESTINATION_LOCK.parse().unwrap(), 84532, lock_id.parse().unwrap(), PROVIDER)
    }

    fn filled(order: &str, destination_lock: B256, amount: &str) -> Value {
        json!({ "type": "Filled", "orderId": order, "destinationLockId": destination_lock, "amount": amount })
    }

    async fn state(test: &TestApp, order: &str) -> (String, String) {
        sqlx::query_as("SELECT o.status, l.status FROM orders o JOIN legs l ON l.order_id = o.id WHERE o.id = $1")
            .bind(order)
            .fetch_one(&test.pool)
            .await
            .unwrap()
    }

    #[sqlx::test]
    async fn a_whole_payment_moves_the_order_step_by_step(pool: PgPool) {
        let test = test_app(pool).await;
        let (order, lock_id) = paying(&test).await;

        let receipt = report(&test, 84532, locked(&order, &lock_id, "12036000")).await;
        assert_eq!(receipt["orderStatus"], "source_locked", "{receipt}");
        assert_eq!(state(&test, &order).await, ("source_locked".into(), "locked".into()));

        let receipt = report(&test, 1874, filled(&order, bobs_lock(&lock_id), "12000000")).await;
        assert_eq!(receipt["orderStatus"], "destination_locked", "{receipt}");

        let receipt = report(&test, 1874, json!({ "type": "Settled", "orderId": order, "amount": "12000000" })).await;
        assert_eq!(receipt["orderStatus"], "paid", "{receipt}");

        let receipt = report(&test, 84532, json!({ "type": "Claimed", "lockId": lock_id })).await;
        assert_eq!(receipt["applied"], true, "{receipt}");
        assert_eq!(state(&test, &order).await, ("paid".into(), "claimed".into()));
        let paid_at: Option<i64> =
            sqlx::query_scalar("SELECT paid_at FROM orders WHERE id = $1").bind(&order).fetch_one(&test.pool).await.unwrap();
        assert!(paid_at.is_some());
    }

    #[sqlx::test]
    async fn a_settled_after_our_own_claim_marked_the_order_paid_is_accepted_and_changes_nothing(pool: PgPool) {
        let test = test_app(pool).await;
        let (order, lock_id) = paying(&test).await;
        report(&test, 1874, filled(&order, bobs_lock(&lock_id), "12000000")).await;
        // As the Relayer does when its claim is mined.
        sqlx::query("UPDATE orders SET status = 'paid', paid_at = 5 WHERE id = $1").bind(&order).execute(&test.pool).await.unwrap();
        let receipt = report(&test, 1874, json!({ "type": "Settled", "orderId": order, "amount": "12000000" })).await;
        assert_eq!((receipt["applied"].clone(), receipt["orderStatus"].clone()), (json!(true), Value::Null), "{receipt}");
        let paid_at: Option<i64> =
            sqlx::query_scalar("SELECT paid_at FROM orders WHERE id = $1").bind(&order).fetch_one(&test.pool).await.unwrap();
        assert_eq!(paid_at, Some(5));
    }

    #[sqlx::test]
    async fn a_fill_reported_before_the_lock_still_counts(pool: PgPool) {
        let test = test_app(pool).await;
        let (order, lock_id) = paying(&test).await;
        let receipt = report(&test, 1874, filled(&order, bobs_lock(&lock_id), "12000000")).await;
        assert_eq!(receipt["orderStatus"], "destination_locked", "{receipt}");
        assert_eq!(state(&test, &order).await, ("destination_locked".into(), "locked".into()));
        // The late `Locked` changes nothing more.
        let receipt = report(&test, 84532, locked(&order, &lock_id, "12036000")).await;
        assert_eq!(receipt["applied"], false, "{receipt}");
    }

    #[sqlx::test]
    async fn only_our_exact_locks_move_anything(pool: PgPool) {
        let test = test_app(pool).await;
        let (order, lock_id) = paying(&test).await;
        let stranger = B256::from(crate::random::bytes32()).to_string();

        for (chain, event) in [
            // A lock someone else opened for our order id.
            (84532, locked(&order, &stranger, "12036000")),
            // Our lock id on the wrong chain, or for less than was signed.
            (11155111, locked(&order, &lock_id, "12036000")),
            (84532, locked(&order, &lock_id, "12000000")),
            // A destination lock that is not Bob's for our source lock, or holds too little.
            (1874, filled(&order, B256::from(crate::random::bytes32()), "12000000")),
            (1874, filled(&order, bobs_lock(&lock_id), "11999999")),
            // Settled before any verified fill.
            (1874, json!({ "type": "Settled", "orderId": order, "amount": "12000000" })),
            (84532, json!({ "type": "Claimed", "lockId": stranger })),
            (84532, json!({ "type": "Refunded", "lockId": stranger })),
        ] {
            let receipt = report(&test, chain, event.clone()).await;
            assert_eq!(receipt["applied"], false, "{event} -> {receipt}");
        }
        assert_eq!(state(&test, &order).await, ("paying".into(), "pending".into()));
    }

    #[sqlx::test]
    async fn a_refunded_source_lock_returns_the_order(pool: PgPool) {
        let test = test_app(pool).await;
        let (order, lock_id) = paying(&test).await;
        report(&test, 84532, locked(&order, &lock_id, "12036000")).await;

        // Bob's own lock going back to him on Whitechain changes nothing here.
        let receipt = report(&test, 1874, json!({ "type": "Refunded", "lockId": lock_id })).await;
        assert_eq!(receipt["applied"], false, "{receipt}");

        let receipt = report(&test, 84532, json!({ "type": "Refunded", "lockId": lock_id })).await;
        assert_eq!(receipt["orderStatus"], "returned", "{receipt}");
        assert_eq!(state(&test, &order).await, ("returned".into(), "returned".into()));
    }
}
