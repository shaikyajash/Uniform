//! Source refunds for locks Bob never filled.
//!
//! The Matchmaker refunds only the source locks Bob filled. When he never fills one (his strategy
//! is empty, the order is over his limit, the Matchmaker is down), the customer's USDC would sit in
//! SourceLock: anyone may call `refund` after expiry, but the customer has no gas. So every
//! `SWEEP_EVERY`, the Order Desk writes a `source_refund` job for each lock our Relayer opened
//! that is past expiry plus `GRACE_SECONDS` and not finished. The grace gives the Matchmaker its
//! turn first; whoever comes second finds the lock refunded and sends nothing.

use std::time::Duration;

use sqlx::PgPool;

use crate::routes::now;

const SWEEP_EVERY: Duration = Duration::from_secs(30);
const GRACE_SECONDS: i64 = 120;

/// Runs forever, in the background of `serve`.
pub async fn run(pool: PgPool) {
    loop {
        match queue_refunds(&pool, now()).await {
            Ok(0) => {}
            Ok(count) => println!("order-desk: queued {count} source refund(s)"),
            Err(error) => eprintln!("order-desk: refund sweep failed: {error}"),
        }
        tokio::time::sleep(SWEEP_EVERY).await;
    }
}

/// Writes one `source_refund` job (`common::jobs::Job::SourceRefund`) per lock that needs one.
/// Never twice for the same lock: a lock whose refund job failed is left for a person to look at.
/// Returns how many it wrote.
pub async fn queue_refunds(pool: &PgPool, now: i64) -> Result<u64, sqlx::Error> {
    let written = sqlx::query(
        "INSERT INTO relayer_jobs (order_id, merchant_id, chain_id, kind, args, created_at, updated_at)
         SELECT l.order_id, o.merchant_id, l.chain_id, 'source_refund',
                jsonb_build_object('kind', 'source_refund', 'lockId', l.lock_id), $1, $1
         FROM legs l JOIN orders o ON o.id = l.order_id
         WHERE l.status IN ('pending', 'locked')
           AND l.expiry + $2 <= $1
           -- Only while the shop has not been paid: after that the lock is Bob's to claim.
           AND o.status IN ('paying', 'source_locked', 'destination_locked')
           -- A lock we know is on chain: reported, or our own lock transaction succeeded.
           AND (l.status = 'locked' OR EXISTS (
                 SELECT 1 FROM relayer_jobs j
                 WHERE j.order_id = l.order_id AND j.kind = 'source_lock' AND j.status = 'confirmed'
                   AND j.args->>'lockId' = l.lock_id))
           AND NOT EXISTS (
                 SELECT 1 FROM relayer_jobs j
                 WHERE j.order_id = l.order_id AND j.kind = 'source_refund' AND j.args->>'lockId' = l.lock_id)
         ON CONFLICT DO NOTHING",
    )
    .bind(now)
    .bind(GRACE_SECONDS)
    .execute(pool)
    .await?;
    Ok(written.rows_affected())
}

#[cfg(test)]
mod tests {
    use alloy::signers::local::PrivateKeySigner;
    use axum::http::StatusCode;
    use common::jobs::Job;
    use serde_json::Value;
    use sqlx::PgPool;

    use super::{GRACE_SECONDS, queue_refunds};
    use crate::routes::{
        legs::tests::{quoted, sign, submit},
        test_support::{call, test_app},
    };

    /// A cross-chain order whose lock job was sent; returns the lock's expiry and id.
    async fn locked(pool: PgPool) -> (PgPool, i64, String) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(&test, &alice).await;
        assert_eq!(call(&test.app, submit(&id, &quote, &sign(&alice, &quote))).await.0, StatusCode::ACCEPTED);
        let (expiry, lock_id): (i64, String) =
            sqlx::query_as("SELECT expiry, lock_id FROM legs").fetch_one(&test.pool).await.unwrap();
        (test.pool, expiry, lock_id)
    }

    async fn refund_jobs(pool: &PgPool) -> Vec<Value> {
        sqlx::query_scalar("SELECT args FROM relayer_jobs WHERE kind = 'source_refund'").fetch_all(pool).await.unwrap()
    }

    #[sqlx::test]
    async fn a_lock_we_opened_gets_one_refund_job_after_expiry_and_grace(pool: PgPool) {
        let (pool, expiry, lock_id) = locked(pool).await;
        sqlx::query("UPDATE relayer_jobs SET status = 'confirmed', tx_hash = $1").bind(&lock_id).execute(&pool).await.unwrap();
        sqlx::query("UPDATE orders SET status = 'source_locked'").execute(&pool).await.unwrap();

        assert_eq!(queue_refunds(&pool, expiry).await.unwrap(), 0, "not before the grace");
        assert_eq!(queue_refunds(&pool, expiry + GRACE_SECONDS).await.unwrap(), 1);
        assert_eq!(queue_refunds(&pool, expiry + GRACE_SECONDS + 60).await.unwrap(), 0, "never twice");

        let jobs = refund_jobs(&pool).await;
        let Job::SourceRefund { lock_id: refunded } = serde_json::from_value(jobs[0].clone()).unwrap() else {
            panic!("expected a source_refund job");
        };
        assert_eq!(refunded.to_string(), lock_id);
    }

    #[sqlx::test]
    async fn locks_that_never_happened_or_were_paid_are_left_alone(pool: PgPool) {
        let (pool, expiry, _) = locked(pool).await;
        let late = expiry + GRACE_SECONDS + 1;
        // Our lock job never succeeded: there is nothing on chain.
        assert_eq!(queue_refunds(&pool, late).await.unwrap(), 0);

        // Reported locked, but the shop was paid: the lock is Bob's to claim.
        sqlx::query("UPDATE legs SET status = 'locked'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE orders SET status = 'paid'").execute(&pool).await.unwrap();
        assert_eq!(queue_refunds(&pool, late).await.unwrap(), 0);

        // Already back with the customer.
        sqlx::query("UPDATE orders SET status = 'source_locked'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE legs SET status = 'returned'").execute(&pool).await.unwrap();
        assert_eq!(queue_refunds(&pool, late).await.unwrap(), 0);
        assert!(refund_jobs(&pool).await.is_empty());
    }
}
