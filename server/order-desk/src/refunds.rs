//! Source refunds for locks Bob never filled.
//!
//! The Matchmaker refunds only the source locks Bob filled. When he never fills one (his strategy
//! is empty, the order is over his limit, the Matchmaker is down), the customer's USDC would sit in
//! SourceLock: anyone may call `refund` after expiry, but the customer has no gas. So every
//! `every`, the Order Desk writes a `source_refund` job for each lock our Relayer opened that is
//! past expiry plus `grace_seconds` and not finished. The grace gives the Matchmaker its turn
//! first; whoever comes second finds the lock refunded and sends nothing. A refund job that failed
//! is written again after `RETRY_AFTER_SECONDS`, for as long as the lock stays open: a refusal
//! (the lock already claimed or refunded) costs no gas, and the Relayer dry-runs before sending.
//! No refund is written while our claim for the order is still being sent: if it is mined, the
//! shop is paid and the lock is Bob's.

use std::time::Duration;

use sqlx::PgPool;

use crate::routes::now;

/// How long after a failed refund job the lock gets a new one.
pub const RETRY_AFTER_SECONDS: i64 = 120;

/// When the sweep runs and how long after a lock's expiry it refunds.
#[derive(Debug, Clone, Copy)]
pub struct Sweep {
    pub every: Duration,
    pub grace_seconds: i64,
}

impl Default for Sweep {
    /// Fast, for the demo: a customer sees their USDC back about half a minute after expiry.
    fn default() -> Sweep {
        Sweep { every: Duration::from_secs(5), grace_seconds: 30 }
    }
}

/// Runs forever, in the background of `serve`.
pub async fn run(pool: PgPool, sweep: Sweep) {
    loop {
        match queue_refunds(&pool, now(), sweep.grace_seconds).await {
            Ok(0) => {}
            Ok(count) => println!("order-desk: queued {count} source refund(s)"),
            Err(error) => eprintln!("order-desk: refund sweep failed: {error}"),
        }
        tokio::time::sleep(sweep.every).await;
    }
}

/// Writes one `source_refund` job (`common::jobs::Job::SourceRefund`) per lock that needs one:
/// none yet, or only failed ones at least `RETRY_AFTER_SECONDS` old.
/// Returns how many it wrote.
pub async fn queue_refunds(pool: &PgPool, now: i64, grace_seconds: i64) -> Result<u64, sqlx::Error> {
    let written = sqlx::query(
        "INSERT INTO relayer_jobs (order_id, merchant_id, chain_id, kind, args, created_at, updated_at)
         SELECT l.order_id, o.merchant_id, l.chain_id, 'source_refund',
                jsonb_build_object('kind', 'source_refund', 'lockId', l.lock_id), $1, $1
         FROM legs l JOIN orders o ON o.id = l.order_id
         WHERE l.status IN ('pending', 'locked')
           AND l.expiry + $2 <= $1
           -- Only while the shop has not been paid: after that the locks of the quote that paid
           -- are Bob's to claim. Any other status may still have an open lock (for example a lock
           -- whose Locked event was never reported). A lock from an earlier quote of a paid order
           -- is not Bob's, so it is refunded too.
           AND (o.status NOT IN ('paid', 'partially_refunded', 'refunded')
                OR l.quote_id <> (SELECT latest.quote_id FROM legs latest WHERE latest.order_id = o.id
                                  ORDER BY latest.id DESC LIMIT 1))
           -- A lock we know is on chain: reported, or our own lock transaction succeeded.
           AND (l.status = 'locked' OR EXISTS (
                 SELECT 1 FROM relayer_jobs j
                 WHERE j.order_id = l.order_id AND j.kind = 'source_lock' AND j.status = 'confirmed'
                   AND j.args->>'lockId' = l.lock_id))
           -- No refund job running or done, and no failure too recent to try again.
           AND NOT EXISTS (
                 SELECT 1 FROM relayer_jobs j
                 WHERE j.order_id = l.order_id AND j.kind = 'source_refund' AND j.args->>'lockId' = l.lock_id
                   AND (j.status <> 'failed' OR j.updated_at > $1 - $3))
           -- Our claim may still be mined; it ends (confirmed or failed) by the lock's expiry.
           AND NOT EXISTS (
                 SELECT 1 FROM relayer_jobs j
                 WHERE j.order_id = l.order_id AND j.kind = 'destination_claim'
                   AND j.status IN ('pending', 'sending', 'sent'))
         ON CONFLICT DO NOTHING",
    )
    .bind(now)
    .bind(grace_seconds)
    .bind(RETRY_AFTER_SECONDS)
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

    use super::{RETRY_AFTER_SECONDS, queue_refunds};
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

    const GRACE_SECONDS: i64 = 30;

    async fn refund_jobs(pool: &PgPool) -> Vec<Value> {
        sqlx::query_scalar("SELECT args FROM relayer_jobs WHERE kind = 'source_refund'").fetch_all(pool).await.unwrap()
    }

    #[sqlx::test]
    async fn a_lock_we_opened_gets_one_refund_job_after_expiry_and_grace(pool: PgPool) {
        let (pool, expiry, lock_id) = locked(pool).await;
        sqlx::query("UPDATE relayer_jobs SET status = 'confirmed', tx_hash = $1").bind(&lock_id).execute(&pool).await.unwrap();
        sqlx::query("UPDATE orders SET status = 'source_locked'").execute(&pool).await.unwrap();

        assert_eq!(queue_refunds(&pool, expiry, GRACE_SECONDS).await.unwrap(), 0, "not before the grace");
        assert_eq!(queue_refunds(&pool, expiry + GRACE_SECONDS, GRACE_SECONDS).await.unwrap(), 1);
        assert_eq!(queue_refunds(&pool, expiry + GRACE_SECONDS + 60, GRACE_SECONDS).await.unwrap(), 0, "never twice");

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
        assert_eq!(queue_refunds(&pool, late, GRACE_SECONDS).await.unwrap(), 0);

        // Reported locked, but the shop was paid: the lock is Bob's to claim.
        sqlx::query("UPDATE legs SET status = 'locked'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE orders SET status = 'paid'").execute(&pool).await.unwrap();
        assert_eq!(queue_refunds(&pool, late, GRACE_SECONDS).await.unwrap(), 0);

        // Already back with the customer.
        sqlx::query("UPDATE orders SET status = 'source_locked'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE legs SET status = 'returned'").execute(&pool).await.unwrap();
        assert_eq!(queue_refunds(&pool, late, GRACE_SECONDS).await.unwrap(), 0);
        assert!(refund_jobs(&pool).await.is_empty());
    }

    #[sqlx::test]
    async fn a_failed_refund_is_tried_again_until_the_lock_is_settled(pool: PgPool) {
        let (pool, expiry, lock_id) = locked(pool).await;
        sqlx::query("UPDATE relayer_jobs SET status = 'confirmed', tx_hash = $1").bind(&lock_id).execute(&pool).await.unwrap();
        sqlx::query("UPDATE orders SET status = 'source_locked'").execute(&pool).await.unwrap();
        let fail_refunds = |at: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query("UPDATE relayer_jobs SET status = 'failed', updated_at = $1 WHERE kind = 'source_refund'")
                    .bind(at)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        };

        let mut at = expiry + GRACE_SECONDS;
        assert_eq!(queue_refunds(&pool, at, GRACE_SECONDS).await.unwrap(), 1);
        fail_refunds(at).await;
        assert_eq!(queue_refunds(&pool, at + RETRY_AFTER_SECONDS - 1, GRACE_SECONDS).await.unwrap(), 0, "not at once");
        for _ in 0..10 {
            at += RETRY_AFTER_SECONDS;
            assert_eq!(queue_refunds(&pool, at, GRACE_SECONDS).await.unwrap(), 1);
            fail_refunds(at).await;
        }
        // Back with the customer: no more tries.
        sqlx::query("UPDATE legs SET status = 'returned'").execute(&pool).await.unwrap();
        assert_eq!(queue_refunds(&pool, at + RETRY_AFTER_SECONDS, GRACE_SECONDS).await.unwrap(), 0);
    }

    #[sqlx::test]
    async fn no_refund_while_our_claim_for_the_order_may_still_be_mined(pool: PgPool) {
        let (pool, expiry, lock_id) = locked(pool).await;
        sqlx::query("UPDATE relayer_jobs SET status = 'confirmed', tx_hash = $1").bind(&lock_id).execute(&pool).await.unwrap();
        sqlx::query("UPDATE orders SET status = 'destination_locked'").execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO relayer_jobs (order_id, merchant_id, chain_id, kind, args, status, tx_hash, created_at, updated_at)
             SELECT order_id, merchant_id, 1874, 'destination_claim', '{}', 'sent', $1, 1, 1 FROM relayer_jobs LIMIT 1",
        )
        .bind(&lock_id)
        .execute(&pool)
        .await
        .unwrap();
        let late = expiry + GRACE_SECONDS;
        assert_eq!(queue_refunds(&pool, late, GRACE_SECONDS).await.unwrap(), 0);

        // The claim ran out unmined: now the customer is refunded.
        sqlx::query("UPDATE relayer_jobs SET status = 'failed' WHERE kind = 'destination_claim'").execute(&pool).await.unwrap();
        assert_eq!(queue_refunds(&pool, late, GRACE_SECONDS).await.unwrap(), 1);
    }

    #[sqlx::test]
    async fn every_lock_of_a_split_is_refunded_even_after_the_order_is_returned(pool: PgPool) {
        use crate::routes::legs::tests::{sign_all, split_quoted, submit_all};
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = split_quoted(&test, &alice).await;
        assert_eq!(call(&test.app, submit_all(&id, &quote, &sign_all(&alice, &quote))).await.0, StatusCode::ACCEPTED);
        sqlx::query("UPDATE relayer_jobs SET status = 'confirmed', tx_hash = args->>'lockId'")
            .execute(&test.pool)
            .await
            .unwrap();
        let expiry: i64 = sqlx::query_scalar("SELECT max(expiry) FROM legs").fetch_one(&test.pool).await.unwrap();
        // Both refunds at once, one per chain.
        assert_eq!(queue_refunds(&test.pool, expiry + GRACE_SECONDS, GRACE_SECONDS).await.unwrap(), 2);

        // The first refund returned the order before the second was sent; the second failed and
        // is written again.
        sqlx::query("UPDATE orders SET status = 'returned'").execute(&test.pool).await.unwrap();
        sqlx::query("UPDATE legs SET status = 'returned' WHERE chain_id = 84532").execute(&test.pool).await.unwrap();
        sqlx::query("UPDATE relayer_jobs SET status = 'failed', updated_at = 0 WHERE kind = 'source_refund' AND chain_id = 11155111")
            .execute(&test.pool)
            .await
            .unwrap();
        let later = expiry + GRACE_SECONDS + RETRY_AFTER_SECONDS;
        assert_eq!(queue_refunds(&test.pool, later, GRACE_SECONDS).await.unwrap(), 1);
        let jobs = refund_jobs(&test.pool).await;
        assert_eq!(jobs.len(), 3);
    }

    #[sqlx::test]
    async fn a_lock_from_an_earlier_quote_is_refunded_even_after_a_new_quote_paid(pool: PgPool) {
        let (pool, expiry, old_lock) = locked(pool).await;
        sqlx::query("UPDATE relayer_jobs SET status = 'confirmed', tx_hash = args->>'lockId'").execute(&pool).await.unwrap();
        // The customer paid with a second quote, whose lock Bob claims.
        sqlx::query(
            "INSERT INTO quotes (id, order_id, path, payer, typed_data, nonce, valid_after, valid_before, expires_at, created_at)
             SELECT 'q_second', order_id, path, payer, typed_data, nonce, valid_after, valid_before, expires_at, created_at
             FROM quotes",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO legs (order_id, quote_id, chain_id, token, amount, payer, lock_id, expiry, status, created_at)
             SELECT order_id, 'q_second', chain_id, token, amount, payer, $1, expiry, 'locked', created_at FROM legs",
        )
        .bind(format!("0x{}", "ab".repeat(32)))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE orders SET status = 'paid'").execute(&pool).await.unwrap();
        assert_eq!(queue_refunds(&pool, expiry + GRACE_SECONDS, GRACE_SECONDS).await.unwrap(), 1);
        let jobs = refund_jobs(&pool).await;
        assert_eq!(jobs[0]["lockId"].as_str(), Some(old_lock.as_str()));
    }

    #[sqlx::test]
    async fn a_paid_order_gets_no_refund_jobs(pool: PgPool) {
        let (pool, expiry, _) = locked(pool).await;
        sqlx::query("UPDATE relayer_jobs SET status = 'confirmed', tx_hash = args->>'lockId'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE orders SET status = 'paid'").execute(&pool).await.unwrap();
        assert_eq!(queue_refunds(&pool, expiry + GRACE_SECONDS, GRACE_SECONDS).await.unwrap(), 0);
    }
}
