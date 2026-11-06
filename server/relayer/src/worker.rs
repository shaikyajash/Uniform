//! One step of the Relayer: take the oldest job no one holds on this chain, move it one stage
//! forward, save. Every job kind runs through the same steps (see `chain::Call`): a payment or a
//! lock carries the customer's note; a claim or a refund acts on a lock.
//!
//! - `pending`: check everything, dry-run, **sign and save** the transaction (`sending`), then
//!   send it (`sent`). The bytes are saved before they go out, so every retry after a timeout or
//!   a crash sends the very same transaction: one job can never become two payments.
//! - `sending` / `sent`: send the saved bytes again if needed and wait for the receipt. Raise the
//!   fee (same nonce) if it is not mined. Follow whichever transaction actually did the job (used
//!   the note, claimed or refunded the lock): someone else may have sent it first.
//!
//! When may a job give the order back to `quoted` so the customer can sign again? Only for a job
//! carrying the customer's note, and only when the note can no longer pay: either nothing was
//! ever sent, or the note has run out unused. Before that, a transaction carrying the note may
//! still be mined, by us or by anyone who saw it.
//!
//! The Relayer never marks an order paid: that comes from the `Paid` event the Matchmaker reports.
//! Every write checks the lease token, so a Relayer that lost the job to another writes nothing.

use alloy::primitives::{B256, Bytes, U256};
use common::{amount::Amount, jobs::Job};
use sqlx::{PgConnection, PgPool, postgres::PgQueryResult};

use crate::chain::{Call, Chain, ChainError, Mined, SignedTx};

/// What the operator sets; see `main.rs` for the environment variables.
#[derive(Debug, Clone)]
pub struct Limits {
    pub chain_id: u64,
    /// The largest order the Relayer pays gas for, in USDT base units (a lock may be 1% more).
    pub max_order: Amount,
    /// Gas (wei) the Relayer may spend for one shop on one UTC day.
    pub daily_gas_wei: U256,
}

/// How long a Relayer holds a job it took. A step makes at most about 8 RPC calls of at most
/// `chain::RPC_TIMEOUT` (10 s) each.
const LEASE_SECONDS: u64 = 120;
/// After an RPC error, wait this long before trying the job again.
pub(crate) const RETRY_SECONDS: u64 = 15;
/// While a transaction is not mined, look again after this long.
pub(crate) const RECEIPT_POLL_SECONDS: u64 = 3;
/// Do not start with a note that runs out sooner than this: it might expire before it is mined.
const MIN_NOTE_LIFE_SECONDS: u64 = 60;
/// Block times and our clock may differ; a note counts as run out only this long after `validBefore`.
pub(crate) const NOTE_EXPIRY_SLACK_SECONDS: u64 = 60;
/// Raise the fee of a transaction not mined after this long, at most `MAX_FEE_BUMPS` times.
pub(crate) const BUMP_AFTER_SECONDS: u64 = 45;
const MAX_FEE_BUMPS: i32 = 3;
/// After this many tries a job that was never sent gives up.
pub(crate) const MAX_ATTEMPTS: i32 = 20;
const SECONDS_PER_DAY: u64 = 86_400;

/// What a step did, for the log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Sent(B256),
    Confirmed(B256),
    Failed(String),
    /// Nothing final yet (waiting for a receipt, an RPC error, a lost lease); looked at again later.
    Waiting(String),
}

pub struct Report {
    pub job_id: i64,
    pub order_id: String,
    pub outcome: Outcome,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    order_id: String,
    merchant_id: i64,
    status: String,
    tx_hash: Option<String>,
    raw_tx: Option<Vec<u8>>,
    tx_nonce: Option<i64>,
    signed_at: Option<i64>,
    fee_bumps: i32,
    attempts: i32,
    lease_token: i64,
    args: serde_json::Value,
}

/// Why a step stopped early.
enum Stop {
    Db(sqlx::Error),
    /// Another Relayer took the job over; it now decides.
    LostLease,
}

impl From<sqlx::Error> for Stop {
    fn from(error: sqlx::Error) -> Stop {
        Stop::Db(error)
    }
}

type Step<T> = Result<T, Stop>;

/// Takes one job and moves it forward. `None` when there is nothing to do right now.
pub async fn step(pool: &PgPool, chain: &impl Chain, limits: &Limits, now: u64) -> Result<Option<Report>, sqlx::Error> {
    let Some(row) = claim(pool, limits.chain_id, now).await? else {
        return Ok(None);
    };
    let job = Taken { pool, id: row.id, token: row.lease_token, order_id: &row.order_id, now };
    let outcome = match job.run(chain, limits, &row).await {
        Ok(outcome) => outcome,
        Err(Stop::LostLease) => Outcome::Waiting("another Relayer took this job over".into()),
        Err(Stop::Db(error)) => return Err(error),
    };
    Ok(Some(Report { job_id: row.id, order_id: row.order_id, outcome }))
}

/// The oldest unfinished job on this chain that no one holds, now held by us with a new random
/// token. `SKIP LOCKED` lets two Relayers run side by side without taking the same job.
async fn claim(pool: &PgPool, chain_id: u64, now: u64) -> Result<Option<Row>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE relayer_jobs
         SET lease_until = $1 + $2, lease_token = floor(random() * 9e18)::bigint,
             attempts = attempts + 1, updated_at = $1
         WHERE id = (
             SELECT id FROM relayer_jobs
             WHERE status IN ('pending', 'sending', 'sent') AND chain_id = $3 AND lease_until <= $1
             ORDER BY created_at, id
             LIMIT 1
             FOR UPDATE SKIP LOCKED)
         RETURNING id, order_id, merchant_id, status, tx_hash, raw_tx, tx_nonce, signed_at, fee_bumps,
                   attempts, lease_token, args",
    )
    .bind(seconds(now))
    .bind(seconds(LEASE_SECONDS))
    .bind(seconds(chain_id))
    .fetch_optional(pool)
    .await
}

/// Our current transaction for a job that has been signed.
struct Ours {
    hash: B256,
    raw: Option<Bytes>,
    nonce: Option<u64>,
    signed_at: u64,
    fee_bumps: i32,
}

/// A job this Relayer holds.
struct Taken<'a> {
    pool: &'a PgPool,
    id: i64,
    token: i64,
    order_id: &'a str,
    now: u64,
}

impl Taken<'_> {
    async fn run(&self, chain: &impl Chain, limits: &Limits, row: &Row) -> Step<Outcome> {
        let never_sent = row.status == "pending";
        let call = match serde_json::from_value::<Job>(row.args.clone()) {
            Ok(job) => Call::from_job(&job),
            Err(error) => Err(format!("unreadable job: {error}")),
        };
        let call = match call {
            Ok(call) => call,
            Err(error) => return self.fail(&error, never_sent).await,
        };
        if never_sent {
            return self.start(chain, &call, row.merchant_id, limits, row.attempts).await;
        }
        let Some(Ok(hash)) = row.tx_hash.as_deref().map(str::parse::<B256>) else {
            return self.fail("unreadable tx hash", false).await;
        };
        let ours = Ours {
            hash,
            raw: row.raw_tx.clone().map(Bytes::from),
            nonce: row.tx_nonce.and_then(|nonce| u64::try_from(nonce).ok()),
            signed_at: row.signed_at.and_then(|at| u64::try_from(at).ok()).unwrap_or(0),
            fee_bumps: row.fee_bumps,
        };
        // Signed and saved, but no node confirmed it has it: send the same bytes again.
        if row.status == "sending"
            && let Some(raw) = &ours.raw
            && chain.broadcast(raw).await.is_ok()
        {
            self.mark_sent().await?;
            return Ok(Outcome::Sent(hash));
        }
        self.watch(chain, &call, row.merchant_id, limits, &ours).await
    }

    /// `pending`: nothing has been signed, so a failure here can reopen the order at once.
    async fn start(
        &self,
        chain: &impl Chain,
        call: &Call,
        merchant_id: i64,
        limits: &Limits,
        attempts: i32,
    ) -> Step<Outcome> {
        if let Some(reason) = self.check_limits(call, merchant_id, limits).await? {
            return self.fail(reason, call.reopens_order()).await;
        }
        // Someone (even the customer, or the Matchmaker) may have done this already.
        match chain.done(call).await {
            Ok(true) => return self.follow_done(chain, call).await,
            Ok(false) => {}
            Err(error) => return self.retry_unsent(call, error, attempts).await,
        }
        if let Err(error) = chain.check(call, self.now).await {
            return self.retry_unsent(call, error, attempts).await;
        }
        let signed = match chain.sign(call, None, 0).await {
            Ok(signed) => signed,
            Err(error) => return self.retry_unsent(call, error, attempts).await,
        };
        self.save_signed(&signed, 0).await?;
        self.send_signed(chain, &signed).await
    }

    /// The checks that need no chain, in the order a shop would want to read them.
    async fn check_limits(&self, call: &Call, merchant_id: i64, limits: &Limits) -> Step<Option<&'static str>> {
        if call.amount_for_limit().is_some_and(|amount| amount > limits.max_order.0) {
            return Ok(Some("order is above the Relayer's limit"));
        }
        // A split order's locks are each smaller than the order: the limit is on the whole order.
        if matches!(call, Call::SourceLock(_)) {
            let amount: Option<String> = sqlx::query_scalar("SELECT amount::text FROM orders WHERE id = $1")
                .bind(self.order_id)
                .fetch_optional(self.pool)
                .await?;
            let amount: Option<Amount> = amount.and_then(|amount| amount.parse().ok());
            if amount.is_none_or(|amount| amount.0 > limits.max_order.0) {
                return Ok(Some("order is above the Relayer's limit"));
            }
        }
        if call.deadline().is_some_and(|deadline| deadline < self.now + MIN_NOTE_LIFE_SECONDS) {
            return Ok(Some("the signed note or the lock runs out too soon"));
        }
        if call.uses_shop_budget() && self.gas_spent_today(merchant_id, limits.chain_id).await? >= limits.daily_gas_wei {
            return Ok(Some("the merchant's gas budget for today is used up"));
        }
        Ok(None)
    }

    /// Nothing was sent: a refusal fails the job (and reopens the order for a note); an RPC error
    /// is retried.
    async fn retry_unsent(&self, call: &Call, error: ChainError, attempts: i32) -> Step<Outcome> {
        let reopen = call.reopens_order();
        match error {
            ChainError::Revert(reason) => self.fail(&reason, reopen).await,
            ChainError::Rpc(reason) if attempts >= MAX_ATTEMPTS => self.fail(&format!("gave up: {reason}"), reopen).await,
            ChainError::Rpc(reason) => self.retry(&reason).await,
        }
    }

    async fn send_signed(&self, chain: &impl Chain, signed: &SignedTx) -> Step<Outcome> {
        match chain.broadcast(&signed.raw).await {
            Ok(()) => {
                self.mark_sent().await?;
                Ok(Outcome::Sent(signed.hash))
            }
            // It may or may not have reached a node. Saved as `sending`; the same bytes go again.
            Err(error) => self.retry(&error.to_string()).await,
        }
    }

    /// `sending` / `sent`: wait for our transaction, or follow the one that did the job.
    async fn watch(
        &self,
        chain: &impl Chain,
        call: &Call,
        merchant_id: i64,
        limits: &Limits,
        ours: &Ours,
    ) -> Step<Outcome> {
        match chain.receipt(ours.hash).await {
            Err(error) => self.retry(&error.to_string()).await,
            Ok(Some(mined)) if mined.success => self.confirm(chain, call, merchant_id, limits, mined, ours.hash).await,
            Ok(Some(mined)) => {
                self.record_reverted(chain, merchant_id, limits, mined).await?;
                self.after_revert(chain, call).await
            }
            Ok(None) => self.not_mined(chain, call, ours).await,
        }
    }

    /// Our transaction reverted. The chain decides what really happened.
    async fn after_revert(&self, chain: &impl Chain, call: &Call) -> Step<Outcome> {
        match chain.done(call).await {
            // Another transaction did it first: follow it. The order is not reopened.
            Ok(true) => self.follow_done(chain, call).await,
            Ok(false) if !call.reopens_order() => self.fail("our transaction reverted", false).await,
            Ok(false) => self.reopen_once_note_runs_out(call, "our transaction reverted").await,
            Err(error) => self.retry(&error.to_string()).await,
        }
    }

    async fn not_mined(&self, chain: &impl Chain, call: &Call, ours: &Ours) -> Step<Outcome> {
        let used = match chain.done(call).await {
            Ok(used) => used,
            Err(error) => return self.retry(&error.to_string()).await,
        };
        if used {
            return match chain.find_done(call).await {
                Ok(Some(other)) if other != ours.hash => self.adopt(other).await,
                Ok(_) => self.poll("mined, waiting for its receipt").await,
                Err(error) => self.retry(&error.to_string()).await,
            };
        }
        if self.ran_out(call) {
            let reason = format!("{} ran out before our transaction was mined", call.what_runs_out());
            return self.fail(&reason, call.reopens_order()).await;
        }
        if self.now >= ours.signed_at + BUMP_AFTER_SECONDS
            && ours.fee_bumps < MAX_FEE_BUMPS
            && let Some(nonce) = ours.nonce
        {
            let bumps = ours.fee_bumps + 1;
            let Ok(signed) = chain.sign(call, Some(nonce), u32::try_from(bumps).unwrap_or(0)).await else {
                return self.poll("not mined yet; could not raise the fee").await;
            };
            self.save_signed(&signed, bumps).await?;
            return self.send_signed(chain, &signed).await;
        }
        // A node may have dropped it; sending the same bytes again is harmless.
        if let Some(raw) = &ours.raw {
            let _ = chain.broadcast(raw).await;
        }
        self.poll("not mined yet").await
    }

    /// The job was done on chain: find the transaction that did it and wait for that one.
    async fn follow_done(&self, chain: &impl Chain, call: &Call) -> Step<Outcome> {
        match chain.find_done(call).await {
            Ok(Some(hash)) => self.adopt(hash).await,
            Ok(None) => self.retry("done on chain but its log is not found yet").await,
            Err(error) => self.retry(&error.to_string()).await,
        }
    }

    async fn reopen_once_note_runs_out(&self, call: &Call, what: &str) -> Step<Outcome> {
        if self.ran_out(call) {
            self.fail(&format!("{what}; the note ran out unused"), true).await
        } else {
            self.retry(&format!("{what}; waiting for the note to run out before reopening the order")).await
        }
    }

    fn ran_out(&self, call: &Call) -> bool {
        call.deadline().is_some_and(|deadline| self.now > deadline + NOTE_EXPIRY_SLACK_SECONDS)
    }

    // ---- Writes. Every one checks the lease token. ----

    async fn save_signed(&self, signed: &SignedTx, fee_bumps: i32) -> Step<()> {
        let result = sqlx::query(
            "UPDATE relayer_jobs
             SET status = 'sending', tx_hash = $3, raw_tx = $4, tx_nonce = $5, fee_bumps = $6, signed_at = $7,
                 updated_at = $7
             WHERE id = $1 AND lease_token = $2",
        )
        .bind(self.id)
        .bind(self.token)
        .bind(hex(signed.hash))
        .bind(signed.raw.to_vec())
        .bind(i64::try_from(signed.nonce).unwrap_or(i64::MAX))
        .bind(fee_bumps)
        .bind(seconds(self.now))
        .execute(self.pool)
        .await;
        held(result)
    }

    async fn mark_sent(&self) -> Step<()> {
        let result = sqlx::query(
            "UPDATE relayer_jobs SET status = 'sent', last_error = NULL, lease_until = $3, updated_at = $4
             WHERE id = $1 AND lease_token = $2",
        )
        .bind(self.id)
        .bind(self.token)
        .bind(seconds(self.now + RECEIPT_POLL_SECONDS))
        .bind(seconds(self.now))
        .execute(self.pool)
        .await;
        held(result)
    }

    /// Follows another transaction that used the note (someone else's, or our earlier one with
    /// the same nonce). Its receipt finishes the job.
    async fn adopt(&self, hash: B256) -> Step<Outcome> {
        let result = sqlx::query(
            "UPDATE relayer_jobs SET status = 'sent', tx_hash = $3, lease_until = 0, updated_at = $4
             WHERE id = $1 AND lease_token = $2",
        )
        .bind(self.id)
        .bind(self.token)
        .bind(hex(hash))
        .bind(seconds(self.now))
        .execute(self.pool)
        .await;
        held(result)?;
        Ok(Outcome::Sent(hash))
    }

    async fn confirm(
        &self,
        chain: &impl Chain,
        call: &Call,
        merchant_id: i64,
        limits: &Limits,
        mined: Mined,
        hash: B256,
    ) -> Step<Outcome> {
        let mut tx = self.pool.begin().await?;
        self.record_gas(&mut tx, chain, merchant_id, limits.chain_id, mined).await?;
        let result = sqlx::query(
            "UPDATE relayer_jobs SET status = 'confirmed', last_error = NULL, updated_at = $3
             WHERE id = $1 AND lease_token = $2",
        )
        .bind(self.id)
        .bind(self.token)
        .bind(seconds(self.now))
        .execute(&mut *tx)
        .await;
        held(result)?;
        self.apply_to_order(&mut tx, call, limits.chain_id, hash).await?;
        tx.commit().await?;
        Ok(Outcome::Confirmed(hash))
    }

    /// What a mined claim or refund means for the order, written at once so the order is right
    /// even when the Matchmaker is down and never reports the event. Its later report changes
    /// nothing more. `paid` is written here and by the Order Desk's `Settled`, so whatever reacts
    /// to a paid order (the shop's webhook) must watch the order's status, not one of the two.
    /// - A destination claim: the shop has the USDT, so the order is `paid`.
    /// - A source refund: the customer has their tokens back, so the leg is `returned`, and so is
    ///   the order once none of its locks is still open, unless the shop was paid.
    async fn apply_to_order(&self, tx: &mut PgConnection, call: &Call, chain_id: u64, hash: B256) -> Step<()> {
        match call {
            Call::DestinationClaim(_) => {
                sqlx::query(
                    "UPDATE orders SET status = 'paid', paid_at = $2
                     WHERE id = (SELECT order_id FROM relayer_jobs WHERE id = $1) AND status = 'destination_locked'",
                )
                .bind(self.id)
                .bind(seconds(self.now))
                .execute(&mut *tx)
                .await?;
            }
            Call::SourceRefund { lock_id } => {
                // The order row first: two refunds of one split order then run one after the
                // other, and the second sees the first's leg as returned.
                sqlx::query("SELECT 1 FROM orders WHERE id = $1 FOR UPDATE")
                    .bind(self.order_id)
                    .execute(&mut *tx)
                    .await?;
                let order_id: Option<String> = sqlx::query_scalar(
                    "UPDATE legs SET status = 'returned', refund_tx = $3
                     WHERE chain_id = $1 AND lock_id = $2 AND status IN ('pending', 'locked')
                     RETURNING order_id",
                )
                .bind(seconds(chain_id))
                .bind(lock_id.to_string())
                .bind(hex(hash))
                .fetch_optional(&mut *tx)
                .await?;
                if let Some(order_id) = order_id {
                    // Open: reported locked, or not yet reported but our lock job for it succeeded.
                    sqlx::query(
                        "UPDATE orders SET status = 'returned'
                         WHERE id = $1 AND status IN ('paying', 'source_locked', 'destination_locked')
                           AND NOT EXISTS (
                               SELECT 1 FROM legs l WHERE l.order_id = $1
                                 AND (l.status = 'locked' OR (l.status = 'pending' AND EXISTS (
                                      SELECT 1 FROM relayer_jobs j WHERE j.order_id = l.order_id
                                        AND j.kind = 'source_lock' AND j.status = 'confirmed'
                                        AND j.args->>'lockId' = l.lock_id))))",
                    )
                    .bind(order_id)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            // A shop refund: `refunded` once the whole payment is back with the customer.
            Call::DirectRefund(call) => {
                let total = call.refunded_before.0 + call.amount.0;
                sqlx::query(
                    "UPDATE orders
                     SET status = CASE WHEN $2::numeric >= amount THEN 'refunded' ELSE 'partially_refunded' END
                     WHERE id = (SELECT order_id FROM relayer_jobs WHERE id = $1)
                       AND status IN ('paid', 'partially_refunded')",
                )
                .bind(self.id)
                .bind(total.to_string())
                .execute(&mut *tx)
                .await?;
            }
            Call::DirectPay(_) | Call::SourceLock(_) => {}
        }
        Ok(())
    }

    async fn record_reverted(&self, chain: &impl Chain, merchant_id: i64, limits: &Limits, mined: Mined) -> Step<()> {
        let mut tx = self.pool.begin().await?;
        self.record_gas(&mut tx, chain, merchant_id, limits.chain_id, mined).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Our gas, once per job: on the job and added to the shop's total for today. Gas paid by
    /// someone else (a transaction we only followed) is not ours.
    async fn record_gas(
        &self,
        tx: &mut PgConnection,
        chain: &impl Chain,
        merchant_id: i64,
        chain_id: u64,
        mined: Mined,
    ) -> Step<()> {
        let not_recorded: Option<bool> = sqlx::query_scalar(
            "SELECT gas_used IS NULL FROM relayer_jobs WHERE id = $1 AND lease_token = $2 FOR UPDATE",
        )
        .bind(self.id)
        .bind(self.token)
        .fetch_optional(&mut *tx)
        .await?;
        match not_recorded {
            None => return Err(Stop::LostLease),
            Some(false) => return Ok(()),
            Some(true) if mined.from != chain.sender() => return Ok(()),
            Some(true) => {}
        }
        let cost = mined.gas_cost.to_string();
        sqlx::query("UPDATE relayer_jobs SET gas_used = $2, gas_cost = $3::numeric WHERE id = $1")
            .bind(self.id)
            .bind(i64::try_from(mined.gas_used).unwrap_or(i64::MAX))
            .bind(&cost)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO gas_budget (merchant_id, chain_id, day, spent) VALUES ($1, $2, $3, $4::numeric)
             ON CONFLICT (merchant_id, chain_id, day) DO UPDATE SET spent = gas_budget.spent + EXCLUDED.spent",
        )
        .bind(merchant_id)
        .bind(seconds(chain_id))
        .bind(seconds(self.now / SECONDS_PER_DAY))
        .bind(&cost)
        .execute(&mut *tx)
        .await?;
        Ok(())
    }

    /// Ends the job. `reopen_order` hands the order back to `quoted` (only from `paying`, and only
    /// when no other lock of the order is open or being sent); pass it only when the note can no
    /// longer pay.
    async fn fail(&self, reason: &str, reopen_order: bool) -> Step<Outcome> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE relayer_jobs SET status = 'failed', last_error = $3, updated_at = $4
             WHERE id = $1 AND lease_token = $2",
        )
        .bind(self.id)
        .bind(self.token)
        .bind(reason)
        .bind(seconds(self.now))
        .execute(&mut *tx)
        .await;
        held(result)?;
        if reopen_order {
            // The order row first: when two lock jobs of a split order fail at once, the second
            // waits for the first and sees its job failed, so the order is reopened.
            sqlx::query("SELECT 1 FROM orders WHERE id = $1 FOR UPDATE")
                .bind(self.order_id)
                .execute(&mut *tx)
                .await?;
            // Not while another lock of the order is open or on its way (a split payment): that
            // money must come back first. The order then ends `returned` with its refund.
            sqlx::query(
                "UPDATE orders SET status = 'quoted' WHERE id = $1 AND status = 'paying'
                 AND NOT EXISTS (SELECT 1 FROM legs WHERE order_id = $1 AND status = 'locked')
                 AND NOT EXISTS (SELECT 1 FROM relayer_jobs
                                 WHERE order_id = $1 AND kind = 'source_lock' AND id <> $2 AND status <> 'failed')",
            )
            .bind(self.order_id)
            .bind(self.id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(Outcome::Failed(reason.to_owned()))
    }

    async fn retry(&self, reason: &str) -> Step<Outcome> {
        self.hold_for(RETRY_SECONDS, Some(reason)).await?;
        Ok(Outcome::Waiting(reason.to_owned()))
    }

    async fn poll(&self, reason: &str) -> Step<Outcome> {
        self.hold_for(RECEIPT_POLL_SECONDS, None).await?;
        Ok(Outcome::Waiting(reason.to_owned()))
    }

    async fn hold_for(&self, wait: u64, reason: Option<&str>) -> Step<()> {
        let result = sqlx::query(
            "UPDATE relayer_jobs SET lease_until = $3, last_error = COALESCE($4, last_error), updated_at = $5
             WHERE id = $1 AND lease_token = $2",
        )
        .bind(self.id)
        .bind(self.token)
        .bind(seconds(self.now + wait))
        .bind(reason)
        .bind(seconds(self.now))
        .execute(self.pool)
        .await;
        held(result)
    }

    async fn gas_spent_today(&self, merchant_id: i64, chain_id: u64) -> Step<U256> {
        let spent: Option<String> = sqlx::query_scalar(
            "SELECT spent::text FROM gas_budget WHERE merchant_id = $1 AND chain_id = $2 AND day = $3",
        )
        .bind(merchant_id)
        .bind(seconds(chain_id))
        .bind(seconds(self.now / SECONDS_PER_DAY))
        .fetch_optional(self.pool)
        .await?;
        Ok(spent.and_then(|text| text.parse::<Amount>().ok()).map_or(U256::ZERO, |amount| amount.0))
    }
}

/// A guarded write that changed no row means the lease token no longer matches.
fn held(result: Result<PgQueryResult, sqlx::Error>) -> Step<()> {
    if result?.rows_affected() == 0 { Err(Stop::LostLease) } else { Ok(()) }
}

/// Postgres has no unsigned integers; unix seconds and chain ids fit in i64 for a long time.
fn seconds(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Lowercase 0x hex, as the `bytes32` domain wants.
fn hex(hash: B256) -> String {
    format!("{hash:#x}")
}

#[cfg(test)]
mod tests;
