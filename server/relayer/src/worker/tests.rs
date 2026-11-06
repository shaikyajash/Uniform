//! The job logic against a real Postgres (`#[sqlx::test]`, the Order Desk's migrations) and a
//! fake chain that behaves like a real one where it matters: a note is used only when a
//! transaction is mined, and of two transactions with the same nonce only one can be mined.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

use alloy::{
    primitives::{Address, B256, Bytes, U256, keccak256},
    signers::{SignerSync, local::PrivateKeySigner},
};
use common::{amount::Amount, jobs::Job};
use sqlx::PgPool;

use super::{
    BUMP_AFTER_SECONDS, Limits, MAX_ATTEMPTS, NOTE_EXPIRY_SLACK_SECONDS, Outcome, RECEIPT_POLL_SECONDS, RETRY_SECONDS,
    step,
};
use crate::chain::{Call, Chain, ChainError, Mined, SignedTx};

const CHAIN_ID: u64 = 2_625;
const NOW: u64 = 1_800_000_000;
const VALID_BEFORE: u64 = NOW + 600;
const PAYER: Address = Address::repeat_byte(0xaa);
const SHOP: Address = Address::repeat_byte(0x5a);
const RELAYER: Address = Address::repeat_byte(0x11);
const SOMEONE_ELSE: Address = Address::repeat_byte(0xee);
const AMOUNT: u64 = 12_000_000;
const GAS_USED: u64 = 90_000;
const GAS_COST: u64 = 900_000_000_000;

fn limits() -> Limits {
    Limits {
        chain_id: CHAIN_ID,
        max_order: Amount::from(100_000_000u64),
        daily_gas_wei: U256::from(10_000_000_000_000_000u64),
    }
}

/// What happens to a broadcast.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Broadcast {
    #[default]
    Accepted,
    /// The node took it, but the reply was lost (a timeout).
    ReplyLost,
    /// The node never got it.
    Down,
}

struct Tx {
    note: B256,
    nonce: u64,
}

#[derive(Default)]
struct State {
    balance: U256,
    blocked: bool,
    dry_run: Option<ChainError>,
    rpc_down: bool,
    broadcast: Broadcast,
    /// Every transaction we signed, by hash.
    signed: HashMap<B256, Tx>,
    mempool: HashSet<B256>,
    /// Every broadcast that reached a node.
    broadcasts: Vec<B256>,
    mined_nonces: HashSet<u64>,
    /// note -> the transaction that used it.
    used_notes: HashMap<B256, B256>,
    receipts: HashMap<B256, Mined>,
    /// When set, signing also changes the job's lease token: another Relayer took the job.
    steal_lease_on_sign: Option<PgPool>,
}

struct FakeChain(Mutex<State>);

impl FakeChain {
    fn new() -> FakeChain {
        FakeChain(Mutex::new(State { balance: U256::from(AMOUNT), ..State::default() }))
    }

    fn with<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        change(&mut self.0.lock().unwrap())
    }

    fn signed_count(&self) -> usize {
        self.with(|state| state.signed.len())
    }

    fn broadcasts(&self) -> Vec<B256> {
        self.with(|state| state.broadcasts.clone())
    }

    /// Mines one of our transactions. It reverts if the note was used already (or `revert`).
    fn mine(&self, hash: B256, revert: bool) {
        self.with(|state| {
            let (note, nonce) = state.signed.get(&hash).map(|tx| (tx.note, tx.nonce)).expect("one of our transactions");
            let same_nonce: Vec<B256> =
                state.signed.iter().filter(|(_, tx)| tx.nonce == nonce).map(|(hash, _)| *hash).collect();
            for other in same_nonce {
                state.mempool.remove(&other);
            }
            state.mined_nonces.insert(nonce);
            let success = !revert && !state.used_notes.contains_key(&note);
            if success {
                state.used_notes.insert(note, hash);
            }
            state.receipts.insert(hash, Mined { from: RELAYER, success, gas_used: GAS_USED, gas_cost: U256::from(GAS_COST) });
        });
    }

    /// Someone else sends the note first and it is mined.
    fn front_run(&self, note: B256) -> B256 {
        let theirs = keccak256(note);
        self.with(|state| {
            state.used_notes.insert(note, theirs);
            state.receipts.insert(theirs, Mined { from: SOMEONE_ELSE, success: true, gas_used: 1, gas_cost: U256::from(1) });
        });
        theirs
    }

    fn down(&self) -> Result<(), ChainError> {
        if self.with(|state| state.rpc_down) { Err(ChainError::Rpc("connection refused".into())) } else { Ok(()) }
    }
}

impl Chain for FakeChain {
    fn sender(&self) -> Address {
        RELAYER
    }

    async fn done(&self, call: &Call) -> Result<bool, ChainError> {
        self.down()?;
        Ok(self.with(|state| state.used_notes.contains_key(&call.done_id())))
    }

    async fn check(&self, call: &Call, _now: u64) -> Result<(), ChainError> {
        self.down()?;
        let amount = match call {
            Call::DirectPay(call) => call.amount.0,
            Call::SourceLock(call) => call.amount.0,
            Call::DestinationClaim(_) | Call::SourceRefund { .. } => U256::ZERO,
        };
        self.with(|state| {
            if state.blocked {
                return Err(ChainError::Revert("the payer is blocked".into()));
            }
            if state.balance < amount {
                return Err(ChainError::Revert("the payer does not have enough".into()));
            }
            state.dry_run.clone().map_or(Ok(()), Err)
        })
    }

    async fn sign(&self, call: &Call, nonce: Option<u64>, fee_bumps: u32) -> Result<SignedTx, ChainError> {
        self.down()?;
        let (signed, steal) = self.with(|state| {
            let pending = state
                .mempool
                .iter()
                .filter_map(|hash| state.signed.get(hash).map(|tx| tx.nonce))
                .chain(state.mined_nonces.iter().copied())
                .max()
                .map_or(0, |highest| highest + 1);
            let nonce = nonce.unwrap_or(pending);
            let note = call.done_id();
            let hash = keccak256([note.as_slice(), &nonce.to_be_bytes(), &fee_bumps.to_be_bytes()].concat());
            state.signed.insert(hash, Tx { note, nonce });
            (SignedTx { raw: Bytes::copy_from_slice(hash.as_slice()), hash, nonce }, state.steal_lease_on_sign.clone())
        });
        if let Some(pool) = steal {
            sqlx::query("UPDATE relayer_jobs SET lease_token = 42").execute(&pool).await.unwrap();
        }
        Ok(signed)
    }

    async fn broadcast(&self, raw: &Bytes) -> Result<(), ChainError> {
        let hash = B256::from_slice(raw);
        self.with(|state| {
            if state.broadcast == Broadcast::Down || state.rpc_down {
                return Err(ChainError::Rpc("connection refused".into()));
            }
            state.broadcasts.push(hash);
            let nonce = state.signed[&hash].nonce;
            if !state.mined_nonces.contains(&nonce) {
                state.mempool.insert(hash);
            }
            match state.broadcast {
                Broadcast::ReplyLost => Err(ChainError::Rpc("no answer within 10s".into())),
                _ => Ok(()),
            }
        })
    }

    async fn receipt(&self, tx_hash: B256) -> Result<Option<Mined>, ChainError> {
        self.down()?;
        Ok(self.with(|state| state.receipts.get(&tx_hash).copied()))
    }

    async fn find_done(&self, call: &Call) -> Result<Option<B256>, ChainError> {
        self.down()?;
        Ok(self.with(|state| state.used_notes.get(&call.done_id()).copied()))
    }
}

/// The note every test job carries.
fn note(order: u8) -> B256 {
    B256::repeat_byte(order)
}

fn order_id(order: u8) -> String {
    format!("{:#x}", B256::repeat_byte(order))
}

/// A shop, a `paying` order and its pending `direct_pay` job; returns the job id.
async fn add_job(pool: &PgPool, order: u8, amount: u64, valid_before: u64) -> i64 {
    let signature = PrivateKeySigner::random().sign_hash_sync(&note(order)).unwrap();
    let job = Job::DirectPay {
        order_id: B256::repeat_byte(order),
        merchant: SHOP,
        amount: Amount::from(amount),
        payer: PAYER,
        valid_after: NOW - 60,
        valid_before,
        nonce: note(order),
        signature: Bytes::from(signature.as_bytes().to_vec()),
    };
    add_order_and_job(pool, order, amount, "paying", &job).await
}

/// A shop, an order in `order_status` and `job` for it, pending; returns the job id.
async fn add_order_and_job(pool: &PgPool, order: u8, amount: u64, order_status: &str, job: &Job) -> i64 {
    sqlx::query(
        "INSERT INTO merchants (name, payout_address, webhook_secret, created_at)
         VALUES ('Kettle & Co', $1, 'whsec_test', 1) ON CONFLICT (payout_address) DO NOTHING",
    )
    .bind(format!("{SHOP:#x}"))
    .execute(pool)
    .await
    .unwrap();
    let merchant_id: i64 = sqlx::query_scalar("SELECT id FROM merchants").fetch_one(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO orders (id, merchant_id, amount, reference, status, path, created_at, expires_at)
         VALUES ($1, $2, $3::numeric, 'shop-123', $5, 'direct', 1, $4)",
    )
    .bind(order_id(order))
    .bind(merchant_id)
    .bind(amount.to_string())
    .bind(i64::try_from(NOW + 1_800).unwrap())
    .bind(order_status)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query_scalar(
        "INSERT INTO relayer_jobs (order_id, merchant_id, chain_id, kind, args, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $6) RETURNING id",
    )
    .bind(order_id(order))
    .bind(merchant_id)
    .bind(i64::try_from(CHAIN_ID).unwrap())
    .bind(job.kind())
    .bind(serde_json::to_value(job).unwrap())
    .bind(i64::try_from(NOW).unwrap())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn add_default_job(pool: &PgPool) -> i64 {
    add_job(pool, 1, AMOUNT, VALID_BEFORE).await
}

/// (job status, order status)
async fn statuses(pool: &PgPool, job: i64) -> (String, String) {
    sqlx::query_as("SELECT j.status, o.status FROM relayer_jobs j JOIN orders o ON o.id = j.order_id WHERE j.id = $1")
        .bind(job)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn gas_spent(pool: &PgPool) -> Option<String> {
    sqlx::query_scalar("SELECT spent::text FROM gas_budget").fetch_optional(pool).await.unwrap()
}

async fn outcome(pool: &PgPool, chain: &FakeChain, now: u64) -> Option<Outcome> {
    step(pool, chain, &limits(), now).await.unwrap().map(|report| report.outcome)
}

async fn sent(pool: &PgPool, chain: &FakeChain, now: u64) -> B256 {
    match outcome(pool, chain, now).await {
        Some(Outcome::Sent(hash)) => hash,
        other => panic!("expected a send, got {other:?}"),
    }
}

fn waiting(result: Option<Outcome>) -> bool {
    matches!(result, Some(Outcome::Waiting(_)))
}

fn failed(result: Option<Outcome>) -> bool {
    matches!(result, Some(Outcome::Failed(_)))
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_job_is_sent_once_then_confirmed_and_its_gas_counted(pool: PgPool) {
    let job = add_default_job(&pool).await;
    let chain = FakeChain::new();

    let hash = sent(&pool, &chain, NOW).await;
    assert_eq!(chain.broadcasts(), vec![hash]);
    assert_eq!(outcome(&pool, &chain, NOW).await, None, "held until the next receipt poll");
    let later = NOW + RECEIPT_POLL_SECONDS;
    assert!(waiting(outcome(&pool, &chain, later).await));

    chain.mine(hash, false);
    assert_eq!(outcome(&pool, &chain, later + RECEIPT_POLL_SECONDS).await, Some(Outcome::Confirmed(hash)));
    assert_eq!(chain.signed_count(), 1, "one transaction for one job");
    assert_eq!(statuses(&pool, job).await, ("confirmed".into(), "paying".into()), "only Paid marks the order paid");
    let (gas_used, job_cost): (i64, String) =
        sqlx::query_as("SELECT gas_used, gas_cost::text FROM relayer_jobs").fetch_one(&pool).await.unwrap();
    assert_eq!((gas_used, job_cost), (GAS_USED as i64, GAS_COST.to_string()));
    assert_eq!(gas_spent(&pool).await, Some(GAS_COST.to_string()));
    assert_eq!(outcome(&pool, &chain, NOW + 3_600).await, None, "a confirmed job is finished");
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_failed_check_sends_nothing_and_reopens_the_order(pool: PgPool) {
    /// (what fails, amount, note runs out at, how the chain is set up)
    type Case = (&'static str, u64, u64, fn(&FakeChain));
    let cases: [Case; 5] = [
        ("enough USDT", AMOUNT, VALID_BEFORE, |chain| chain.with(|state| state.balance = U256::from(AMOUNT - 1))),
        ("blocked", AMOUNT, VALID_BEFORE, |chain| chain.with(|state| state.blocked = true)),
        ("dry run", AMOUNT, VALID_BEFORE, |chain| {
            chain.with(|state| state.dry_run = Some(ChainError::Revert("InvalidMerchant".into())))
        }),
        ("over the limit", 100_000_001, VALID_BEFORE, |_| {}),
        ("note runs out", AMOUNT, NOW + 30, |_| {}),
    ];
    for (order, (name, amount, valid_before, set_up)) in (1u8..).zip(cases) {
        let job = add_job(&pool, order, amount, valid_before).await;
        let chain = FakeChain::new();
        set_up(&chain);
        let result = outcome(&pool, &chain, NOW).await;
        assert!(failed(result.clone()), "{name}: {result:?}");
        assert_eq!(chain.signed_count(), 0, "{name}: nothing may be signed");
        assert_eq!(statuses(&pool, job).await, ("failed".into(), "quoted".into()), "{name}");
    }
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_shop_over_its_daily_gas_budget_gets_nothing_sent(pool: PgPool) {
    let job = add_default_job(&pool).await;
    sqlx::query("INSERT INTO gas_budget (merchant_id, chain_id, day, spent) SELECT id, $1, $2, $3::numeric FROM merchants")
        .bind(i64::try_from(CHAIN_ID).unwrap())
        .bind(i64::try_from(NOW / 86_400).unwrap())
        .bind(limits().daily_gas_wei.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let chain = FakeChain::new();
    assert!(failed(outcome(&pool, &chain, NOW).await));
    assert_eq!(chain.signed_count(), 0);
    assert_eq!(statuses(&pool, job).await.0, "failed");
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn an_rpc_error_before_sending_waits_then_gives_up(pool: PgPool) {
    let job = add_default_job(&pool).await;
    let chain = FakeChain::new();
    chain.with(|state| state.rpc_down = true);

    assert!(waiting(outcome(&pool, &chain, NOW).await));
    assert_eq!(statuses(&pool, job).await.0, "pending");
    assert_eq!(outcome(&pool, &chain, NOW + 1).await, None, "not before the retry wait");

    sqlx::query("UPDATE relayer_jobs SET attempts = $1").bind(MAX_ATTEMPTS).execute(&pool).await.unwrap();
    let result = outcome(&pool, &chain, NOW + RETRY_SECONDS).await;
    assert!(matches!(result, Some(Outcome::Failed(ref reason)) if reason.starts_with("gave up")), "{result:?}");
    assert_eq!(statuses(&pool, job).await.1, "quoted");
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_send_whose_reply_was_lost_is_sent_again_as_the_same_transaction(pool: PgPool) {
    let job = add_default_job(&pool).await;
    let chain = FakeChain::new();
    chain.with(|state| state.broadcast = Broadcast::ReplyLost);
    assert!(waiting(outcome(&pool, &chain, NOW).await));
    assert_eq!(statuses(&pool, job).await.0, "sending");

    chain.with(|state| state.broadcast = Broadcast::Accepted);
    let hash = sent(&pool, &chain, NOW + RETRY_SECONDS).await;
    assert_eq!(chain.broadcasts(), vec![hash, hash], "the same bytes twice");
    assert_eq!(chain.signed_count(), 1, "never a second transaction");

    chain.mine(hash, false);
    assert_eq!(outcome(&pool, &chain, NOW + RETRY_SECONDS + RECEIPT_POLL_SECONDS).await, Some(Outcome::Confirmed(hash)));
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_send_that_never_reached_a_node_is_sent_again_as_the_same_transaction(pool: PgPool) {
    add_default_job(&pool).await;
    let chain = FakeChain::new();
    chain.with(|state| state.broadcast = Broadcast::Down);
    assert!(waiting(outcome(&pool, &chain, NOW).await));
    assert!(chain.broadcasts().is_empty());

    chain.with(|state| state.broadcast = Broadcast::Accepted);
    let hash = sent(&pool, &chain, NOW + RETRY_SECONDS).await;
    assert_eq!((chain.broadcasts(), chain.signed_count()), (vec![hash], 1));
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn when_someone_else_pays_first_we_follow_their_payment_and_never_reopen(pool: PgPool) {
    let job = add_default_job(&pool).await;
    let chain = FakeChain::new();
    let ours = sent(&pool, &chain, NOW).await;

    let theirs = chain.front_run(note(1));
    chain.mine(ours, false); // reverts: the note is used
    let later = NOW + RECEIPT_POLL_SECONDS;
    assert_eq!(outcome(&pool, &chain, later).await, Some(Outcome::Sent(theirs)));
    assert_eq!(statuses(&pool, job).await, ("sent".into(), "paying".into()), "the order is paid, not reopened");

    assert_eq!(outcome(&pool, &chain, later).await, Some(Outcome::Confirmed(theirs)));
    assert_eq!(gas_spent(&pool).await, Some(GAS_COST.to_string()), "only our own gas, once");
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_note_already_used_before_we_start_is_followed_not_sent(pool: PgPool) {
    let job = add_default_job(&pool).await;
    let chain = FakeChain::new();
    let theirs = chain.front_run(note(1));
    assert_eq!(outcome(&pool, &chain, NOW).await, Some(Outcome::Sent(theirs)));
    assert_eq!(chain.signed_count(), 0);
    assert_eq!(outcome(&pool, &chain, NOW).await, Some(Outcome::Confirmed(theirs)));
    assert_eq!(statuses(&pool, job).await, ("confirmed".into(), "paying".into()));
    assert_eq!(gas_spent(&pool).await, None, "we paid no gas");
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn after_our_transaction_reverts_the_order_reopens_only_once_the_note_runs_out(pool: PgPool) {
    let job = add_default_job(&pool).await;
    let chain = FakeChain::new();
    let ours = sent(&pool, &chain, NOW).await;
    chain.mine(ours, true);

    let result = outcome(&pool, &chain, NOW + RECEIPT_POLL_SECONDS).await;
    assert!(waiting(result.clone()), "{result:?}");
    assert_eq!(statuses(&pool, job).await, ("sent".into(), "paying".into()), "the note could still be used");

    let note_gone = VALID_BEFORE + NOTE_EXPIRY_SLACK_SECONDS + 1;
    assert!(failed(outcome(&pool, &chain, note_gone).await));
    assert_eq!(statuses(&pool, job).await, ("failed".into(), "quoted".into()));
    assert_eq!(gas_spent(&pool).await, Some(GAS_COST.to_string()), "the reverted gas is counted once");
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_transaction_never_mined_fails_only_after_the_note_runs_out(pool: PgPool) {
    let job = add_default_job(&pool).await;
    let chain = FakeChain::new();
    sent(&pool, &chain, NOW).await;
    sqlx::query("UPDATE relayer_jobs SET fee_bumps = 3").execute(&pool).await.unwrap();
    assert!(waiting(outcome(&pool, &chain, VALID_BEFORE).await));
    assert_eq!(statuses(&pool, job).await.1, "paying");

    assert!(failed(outcome(&pool, &chain, VALID_BEFORE + NOTE_EXPIRY_SLACK_SECONDS + 1).await));
    assert_eq!(statuses(&pool, job).await, ("failed".into(), "quoted".into()));
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_slow_transaction_gets_a_higher_fee_and_whichever_one_is_mined_counts(pool: PgPool) {
    add_default_job(&pool).await;
    let chain = FakeChain::new();
    let first = sent(&pool, &chain, NOW).await;

    let bumped = sent(&pool, &chain, NOW + BUMP_AFTER_SECONDS).await;
    assert_ne!(first, bumped);
    let nonces: HashSet<u64> = chain.with(|state| state.signed.values().map(|tx| tx.nonce).collect());
    assert_eq!(nonces.len(), 1, "the higher fee reuses the nonce, so only one can be mined");

    chain.mine(first, false);
    let later = NOW + BUMP_AFTER_SECONDS + RECEIPT_POLL_SECONDS;
    assert_eq!(outcome(&pool, &chain, later).await, Some(Outcome::Sent(first)));
    assert_eq!(outcome(&pool, &chain, later).await, Some(Outcome::Confirmed(first)));
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_relayer_that_lost_its_lease_sends_nothing(pool: PgPool) {
    let job = add_default_job(&pool).await;
    let chain = FakeChain::new();
    chain.with(|state| state.steal_lease_on_sign = Some(pool.clone()));
    assert_eq!(outcome(&pool, &chain, NOW).await, Some(Outcome::Waiting("another Relayer took this job over".into())));
    assert!(chain.broadcasts().is_empty());
    assert_eq!(statuses(&pool, job).await.0, "pending", "nothing written by the Relayer that lost the job");
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_job_on_another_chain_is_left_alone(pool: PgPool) {
    add_default_job(&pool).await;
    sqlx::query("UPDATE relayer_jobs SET chain_id = 1").execute(&pool).await.unwrap();
    assert_eq!(outcome(&pool, &FakeChain::new(), NOW).await, None);
}

// ---- The cross-chain jobs: the same machinery, with what differs checked here. ----

fn lock_job(order: u8, amount: u64) -> Job {
    let signature = PrivateKeySigner::random().sign_hash_sync(&note(order)).unwrap();
    Job::SourceLock {
        params: common::locks::SourceLockParams {
            order_id: B256::repeat_byte(order),
            receiver: Address::repeat_byte(0xb0),
            min_usdc_out: Amount::from(amount),
            hashlock: B256::repeat_byte(0x42),
            expiry: NOW + 1_200,
            out_token: Address::ZERO,
        },
        payer: PAYER,
        token: Address::repeat_byte(0xc0),
        amount: Amount::from(amount),
        valid_after: NOW - 60,
        valid_before: NOW + 300,
        lock_id: note(order),
        signature: Bytes::from(signature.as_bytes().to_vec()),
    }
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_lock_with_bobs_fee_on_a_full_size_order_is_sent_and_confirmed(pool: PgPool) {
    // 100 USDT is the limit; the customer locks 100.3 USDC for it.
    let job = add_order_and_job(&pool, 1, 100_000_000, "paying", &lock_job(1, 100_300_000)).await;
    let chain = FakeChain::new();
    chain.with(|state| state.balance = U256::from(100_300_000u64));
    let hash = sent(&pool, &chain, NOW).await;
    chain.mine(hash, false);
    assert_eq!(outcome(&pool, &chain, NOW + RECEIPT_POLL_SECONDS).await, Some(Outcome::Confirmed(hash)));
    assert_eq!(statuses(&pool, job).await, ("confirmed".into(), "paying".into()), "only Locked moves the order");
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_refused_lock_reopens_the_order_so_the_customer_can_sign_again(pool: PgPool) {
    let job = add_order_and_job(&pool, 1, AMOUNT, "paying", &lock_job(1, 12_036_000)).await;
    let chain = FakeChain::new();
    chain.with(|state| state.dry_run = Some(ChainError::Revert("too little time left on the lock".into())));
    assert!(failed(outcome(&pool, &chain, NOW).await));
    assert_eq!(statuses(&pool, job).await, ("failed".into(), "quoted".into()));
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_refused_claim_sends_nothing_and_leaves_the_order_alone(pool: PgPool) {
    let claim = Job::DestinationClaim {
        lock_id: note(1),
        secret: B256::repeat_byte(7),
        merchant: SHOP,
        min_amount: Amount::from(AMOUNT),
        deadline: NOW + 1_200,
    };
    let job = add_order_and_job(&pool, 1, AMOUNT, "destination_locked", &claim).await;
    let chain = FakeChain::new();
    chain.with(|state| state.dry_run = Some(ChainError::Revert("the shop's lock pays someone else".into())));
    let result = outcome(&pool, &chain, NOW).await;
    assert!(matches!(result, Some(Outcome::Failed(ref reason)) if reason.contains("someone else")), "{result:?}");
    assert_eq!(chain.signed_count(), 0, "the secret never left");
    assert_eq!(statuses(&pool, job).await, ("failed".into(), "destination_locked".into()));
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_claim_whose_transaction_reverts_fails_at_once(pool: PgPool) {
    let claim = Job::DestinationClaim {
        lock_id: note(1),
        secret: B256::repeat_byte(7),
        merchant: SHOP,
        min_amount: Amount::from(AMOUNT),
        deadline: NOW + 1_200,
    };
    let job = add_order_and_job(&pool, 1, AMOUNT, "destination_locked", &claim).await;
    let chain = FakeChain::new();
    let ours = sent(&pool, &chain, NOW).await;
    chain.mine(ours, true);
    assert!(failed(outcome(&pool, &chain, NOW + RECEIPT_POLL_SECONDS).await));
    assert_eq!(statuses(&pool, job).await, ("failed".into(), "destination_locked".into()));
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_lock_the_matchmaker_already_refunded_is_followed_not_sent(pool: PgPool) {
    let job = add_order_and_job(&pool, 1, AMOUNT, "source_locked", &Job::SourceRefund { lock_id: note(1) }).await;
    let chain = FakeChain::new();
    let theirs = chain.front_run(note(1));
    assert_eq!(outcome(&pool, &chain, NOW).await, Some(Outcome::Sent(theirs)));
    assert_eq!(outcome(&pool, &chain, NOW).await, Some(Outcome::Confirmed(theirs)));
    assert_eq!(chain.signed_count(), 0);
    assert_eq!(statuses(&pool, job).await.0, "confirmed");
    assert_eq!(gas_spent(&pool).await, None, "we paid no gas");
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_refund_never_runs_out_so_a_slow_one_keeps_waiting(pool: PgPool) {
    let job = add_order_and_job(&pool, 1, AMOUNT, "source_locked", &Job::SourceRefund { lock_id: note(1) }).await;
    let chain = FakeChain::new();
    let hash = sent(&pool, &chain, NOW).await;
    sqlx::query("UPDATE relayer_jobs SET fee_bumps = 3").execute(&pool).await.unwrap();
    assert!(waiting(outcome(&pool, &chain, NOW + 86_400).await));
    chain.mine(hash, false);
    assert_eq!(outcome(&pool, &chain, NOW + 86_400 + RECEIPT_POLL_SECONDS).await, Some(Outcome::Confirmed(hash)));
    assert_eq!(statuses(&pool, job).await, ("confirmed".into(), "source_locked".into()));
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_refund_is_sent_even_when_the_shop_has_spent_its_gas_budget(pool: PgPool) {
    let job = add_order_and_job(&pool, 1, AMOUNT, "source_locked", &Job::SourceRefund { lock_id: note(1) }).await;
    sqlx::query("INSERT INTO gas_budget (merchant_id, chain_id, day, spent) SELECT id, $1, $2, $3::numeric FROM merchants")
        .bind(i64::try_from(CHAIN_ID).unwrap())
        .bind(i64::try_from(NOW / 86_400).unwrap())
        .bind(limits().daily_gas_wei.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let chain = FakeChain::new();
    let hash = sent(&pool, &chain, NOW).await;
    chain.mine(hash, false);
    assert_eq!(outcome(&pool, &chain, NOW + RECEIPT_POLL_SECONDS).await, Some(Outcome::Confirmed(hash)));
    assert_eq!(statuses(&pool, job).await.0, "confirmed");
}

#[sqlx::test(migrations = "../order-desk/migrations")]
async fn a_claim_is_sent_even_when_the_shop_has_spent_its_gas_budget(pool: PgPool) {
    let claim = Job::DestinationClaim {
        lock_id: note(1),
        secret: B256::repeat_byte(7),
        merchant: SHOP,
        min_amount: Amount::from(AMOUNT),
        deadline: NOW + 1_200,
    };
    let job = add_order_and_job(&pool, 1, AMOUNT, "destination_locked", &claim).await;
    sqlx::query("INSERT INTO gas_budget (merchant_id, chain_id, day, spent) SELECT id, $1, $2, $3::numeric FROM merchants")
        .bind(i64::try_from(CHAIN_ID).unwrap())
        .bind(i64::try_from(NOW / 86_400).unwrap())
        .bind(limits().daily_gas_wei.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let chain = FakeChain::new();
    let hash = sent(&pool, &chain, NOW).await;
    chain.mine(hash, false);
    assert_eq!(outcome(&pool, &chain, NOW + RECEIPT_POLL_SECONDS).await, Some(Outcome::Confirmed(hash)));
    assert_eq!(statuses(&pool, job).await.0, "confirmed");
}
