//! `POST /v1/orders/{id}/secret`: the pay page reveals the customer's secret once Bob's USDT is
//! locked for the shop (the pay page checks that lock on chain itself first).
//!
//! The Order Desk checks the secret against the order's hashlock and writes a `destination_claim`
//! job: the Relayer claims the destination lock, which pays the shop, and pays the gas. The order
//! becomes `paid` from the `Settled` event.

use alloy::primitives::B256;
use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
};
use common::{amount::Amount, jobs::Job, locks::hashlock_of, status::OrderStatus};
use serde::{Deserialize, Serialize};

use super::{AppState, normalize_order_id, now};
use crate::error::ApiError;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretRequest {
    /// 32 bytes of 0x hex whose keccak256 is the order's hashlock.
    secret: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretAccepted {
    status: OrderStatus,
    job_id: i64,
}

#[derive(sqlx::FromRow)]
struct LockedOrder {
    status: String,
    amount: String,
    hashlock: Option<String>,
    destination_lock_id: Option<String>,
    merchant_id: i64,
    payout_address: String,
    /// The latest locked leg's expiry: the destination lock ends before it.
    leg_expiry: Option<i64>,
}

pub async fn reveal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<SecretRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<SecretAccepted>), ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let order_id = normalize_order_id(&id).ok_or_else(|| ApiError::not_found("order"))?;
    let secret: B256 =
        body.secret.parse().map_err(|_| ApiError::bad_request("bad_secret", "secret must be 32 bytes of 0x hex"))?;

    let order: LockedOrder = sqlx::query_as(
        "SELECT o.status, o.amount::text AS amount, o.hashlock, o.destination_lock_id, o.merchant_id, m.payout_address,
                (SELECT l.expiry FROM legs l WHERE l.order_id = o.id AND l.status <> 'pending'
                 ORDER BY l.id DESC LIMIT 1) AS leg_expiry
         FROM orders o JOIN merchants m ON m.id = o.merchant_id
         WHERE o.id = $1",
    )
    .bind(&order_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("order"))?;

    let status: OrderStatus = order.status.parse().map_err(ApiError::internal)?;
    if status != OrderStatus::DestinationLocked {
        return Err(ApiError::conflict(
            "not_ready",
            format!("order is {status}; the secret is only taken once the merchant's lock is open"),
        ));
    }
    let hashlock: B256 = order.hashlock.as_deref().unwrap_or_default().parse().map_err(ApiError::internal)?;
    if hashlock_of(secret) != hashlock {
        return Err(ApiError::bad_request("wrong_secret", "this secret does not match the order's hashlock"));
    }
    let (Some(lock_id), Some(deadline)) = (order.destination_lock_id, order.leg_expiry) else {
        return Err(ApiError::internal("destination_locked order without its lock or leg"));
    };
    let job = Job::DestinationClaim {
        lock_id: lock_id.parse().map_err(ApiError::internal)?,
        secret,
        merchant: order.payout_address.parse().map_err(ApiError::internal)?,
        min_amount: order.amount.parse::<Amount>().map_err(ApiError::internal)?,
        deadline: u64::try_from(deadline).map_err(ApiError::internal)?,
    };
    let chain_id = i64::try_from(state.chains.shop_chain().chain_id).map_err(ApiError::internal)?;

    let mut tx = state.pool.begin().await?;
    let job_id = super::insert_job(&mut tx, &order_id, order.merchant_id, chain_id, &job, now()).await?;
    tx.commit().await?;
    Ok((StatusCode::ACCEPTED, Json(SecretAccepted { status, job_id })))
}

#[cfg(test)]
mod tests {
    use alloy::signers::local::PrivateKeySigner;
    use axum::http::StatusCode;
    use common::jobs::Job;
    use serde_json::{Value, json};
    use sqlx::PgPool;

    use crate::routes::{
        legs::tests::{SECRET, post, quoted, sign, submit},
        test_support::{TestApp, call, test_app},
    };

    const DESTINATION_LOCK_ID: &str = "0x5555555555555555555555555555555555555555555555555555555555555555";

    /// A cross-chain order whose source leg is locked and whose destination lock is open.
    async fn destination_locked(test: &TestApp) -> String {
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(test, &alice).await;
        assert_eq!(call(&test.app, submit(&id, &quote, &sign(&alice, &quote))).await.0, StatusCode::ACCEPTED);
        sqlx::query("UPDATE relayer_jobs SET status = 'confirmed', tx_hash = $1").bind(DESTINATION_LOCK_ID).execute(&test.pool).await.unwrap();
        sqlx::query("UPDATE legs SET status = 'locked'").execute(&test.pool).await.unwrap();
        sqlx::query("UPDATE orders SET status = 'destination_locked', destination_lock_id = $2 WHERE id = $1")
            .bind(&id)
            .bind(DESTINATION_LOCK_ID)
            .execute(&test.pool)
            .await
            .unwrap();
        id
    }

    fn reveal(id: &str, secret: &str) -> axum::http::Request<axum::body::Body> {
        post(format!("/v1/orders/{id}/secret"), json!({ "secret": secret }))
    }

    #[sqlx::test]
    async fn the_right_secret_becomes_a_claim_job_for_the_shop(pool: PgPool) {
        let test = test_app(pool).await;
        let id = destination_locked(&test).await;
        let (status, reply) = call(&test.app, reveal(&id, SECRET)).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{reply}");

        let (kind, chain_id, args): (String, i64, Value) =
            sqlx::query_as("SELECT kind, chain_id, args FROM relayer_jobs WHERE id = $1")
                .bind(reply["jobId"].as_i64().unwrap())
                .fetch_one(&test.pool)
                .await
                .unwrap();
        assert_eq!((kind.as_str(), chain_id), ("destination_claim", 1874));
        let Job::DestinationClaim { lock_id, secret, merchant, min_amount, deadline } = serde_json::from_value(args).unwrap() else {
            panic!("expected a destination_claim job");
        };
        assert_eq!(lock_id.to_string(), DESTINATION_LOCK_ID);
        assert_eq!(secret.to_string(), SECRET);
        assert_eq!(format!("{merchant:#x}"), "0x5a30000000000000000000000000000000000001");
        assert_eq!(min_amount.to_string(), "12000000");
        let expiry: i64 = sqlx::query_scalar("SELECT expiry FROM legs").fetch_one(&test.pool).await.unwrap();
        assert_eq!(deadline, u64::try_from(expiry).unwrap());

        let (status, reply) = call(&test.app, reveal(&id, SECRET)).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("already_submitted")));
    }

    #[sqlx::test]
    async fn a_wrong_secret_or_an_order_not_ready_is_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let id = destination_locked(&test).await;
        let wrong = format!("0x{}", "08".repeat(32));
        let (status, reply) = call(&test.app, reveal(&id, &wrong)).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("wrong_secret")));
        let (status, reply) = call(&test.app, reveal(&id, "0x1234")).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_secret")));

        sqlx::query("UPDATE orders SET status = 'source_locked'").execute(&test.pool).await.unwrap();
        let (status, reply) = call(&test.app, reveal(&id, SECRET)).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("not_ready")));
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM relayer_jobs WHERE kind = 'destination_claim'")
            .fetch_one(&test.pool)
            .await
            .unwrap();
        assert_eq!(jobs, 0);
    }
}
