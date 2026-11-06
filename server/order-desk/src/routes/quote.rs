//! `POST /v1/orders/{id}/quote`: the pay page asks what the customer must sign.
//!
//! - `{payer}`: Path A, USDT on Whitechain through DirectPay.
//! - `{payer, hashlock, chainId?}`: the cross-chain path. The customer locks USDC on `chainId`
//!   (default: the one customer chain with a SourceLock) for Bob, under the hashlock of a secret
//!   only the pay page knows.
//! - `{payer, hashlock, legs: [{chainId, amount?}, ...]}`: the split path. The customer locks part
//!   of the USDC on each chain, all under one hashlock; Bob fills only once every lock is in. The
//!   amounts are USDC and must add up to the order plus Bob's fee; one may be left out to take the
//!   rest.
//!
//! The pay page picks the path from balances it reads itself; the Order Desk connects to no chain,
//! and the Relayer checks the balance before it spends gas.

use alloy::{
    hex,
    primitives::{Address, B256, U256},
};
use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
};
use common::{
    amount::Amount,
    calibur,
    chains::{Chain, Token},
    locks::{SourceLockParams, source_lock_id, with_provider_fee},
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
const USDC: &str = "USDC";
/// SourceLock takes only USDC and EURC, both with 6 decimals (checked against chains.json).
const USDC_DECIMALS: u8 = 6;
/// A token paid through a Calibur batch is quoted this much above its value at the market price:
/// 2% above the Matchmaker's own buffer (1%), so a price move while the customer signs and the
/// lock waits still fills.
const QUOTE_BUFFER_BPS: u64 = 300;
/// The least a split leg may lock (0.1 USDC), so a split cannot make the Relayer pay gas for dust.
const MIN_SPLIT_LEG: u64 = 100_000;
/// `validAfter` = now - 60, so a customer whose clock runs a little behind can still sign (SPEC).
const CLOCK_SLACK_SECONDS: i64 = 60;
/// Path A: `validBefore` = now + 600 (SPEC). DirectPay rejects anything more than 1 hour ahead.
const NOTE_LIFETIME_SECONDS: i64 = 600;
/// Path B: `validBefore` = now + 300 (SPEC). SourceLock wants it at least 15 minutes before the
/// lock's expiry (now + 1200).
const LOCK_NOTE_LIFETIME_SECONDS: i64 = 300;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRequest {
    /// The customer's wallet.
    payer: String,
    /// Cross-chain only: `keccak256(secret)`, from the pay page. The lock id (the note's nonce)
    /// is built from it, so it must be known before the customer signs.
    hashlock: Option<String>,
    /// Cross-chain only: the chain the customer pays from.
    chain_id: Option<u64>,
    /// Split only: the chains the customer pays from, and how much USDC on each.
    legs: Option<Vec<LegRequest>>,
    /// Cross-chain only: the token the customer pays with, by symbol in chains.json. `USDC` when
    /// left out. A token with a `priceId` (WETH, WBTC) is locked through a Calibur batch.
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegRequest {
    chain_id: u64,
    /// USDC, as a decimal string. Left out on at most one leg, which then takes the rest.
    amount: Option<String>,
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
    /// One note per chain the customer pays from: exactly one on Paths A and B, one per leg on a
    /// split. Each is signed on its own chain.
    to_sign: Vec<ToSign>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToSign {
    chain_id: u64,
    token: String,
    amount: String,
    typed_data: Value,
    /// Cross-chain: the source lock the note opens (also its nonce).
    #[serde(skip_serializing_if = "Option::is_none")]
    lock_id: Option<B256>,
    /// `note`: an EIP-3009 note. `batch`: a Calibur batch (approve + lock), signed by the wallet
    /// once it is delegated to `calibur` (EIP-7702).
    signing: &'static str,
    /// Batch only: the token's address and decimals, and the Calibur delegate.
    #[serde(skip_serializing_if = "Option::is_none")]
    token_address: Option<Address>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decimals: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    calibur: Option<Address>,
}

/// The source lock a cross-chain quote plans: stored as `quotes.leg`, read back by `/legs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedLeg {
    pub chain_id: u64,
    pub token: Address,
    pub amount: Amount,
    pub params: SourceLockParams,
    pub lock_id: B256,
    /// Split quotes only: the note that opens this lock. A single-lock quote keeps its note in
    /// `quotes.typed_data`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_data: Option<Value>,
    /// A lock through the customer's Calibur account: the batch's nonce and deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<PlannedBatch>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedBatch {
    pub nonce: U256,
    pub deadline: u64,
}

/// The source locks a quote plans: one object for a cross-chain quote, a list for a split.
pub fn planned_legs(leg: Value, quote_typed_data: &Value) -> Result<Vec<PlannedLeg>, serde_json::Error> {
    if leg.is_array() {
        return serde_json::from_value(leg);
    }
    let mut one: PlannedLeg = serde_json::from_value(leg)?;
    one.typed_data.get_or_insert_with(|| quote_typed_data.clone());
    Ok(vec![one])
}

#[derive(sqlx::FromRow)]
struct PayableOrder {
    amount: String,
    status: String,
    expires_at: i64,
    payout_address: String,
}

/// What both paths need from the request and the order.
struct Quoting {
    order_id: String,
    order_bytes: B256,
    payer: Address,
    amount: Amount,
    merchant: Address,
    order_expires_at: i64,
    now: i64,
}

/// One note for the customer to sign.
struct Planned {
    note: Note,
    typed_data: Value,
    to_sign: ToSign,
    leg: Option<PlannedLeg>,
}

/// One quote, ready to store and return.
struct Priced {
    path: PayPath,
    /// At least one; the first is stored in `quotes.typed_data` and `quotes.nonce`.
    notes: Vec<Planned>,
    total: String,
    fee: String,
    hashlock: Option<B256>,
}

pub async fn quote(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<QuoteRequest>, JsonRejection>,
) -> Result<Json<QuoteView>, ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let payer: Address =
        body.payer.parse().map_err(|_| ApiError::bad_request("bad_payer", "payer must be a 0x wallet address"))?;
    let hashlock = body.hashlock.as_deref().map(parse_hashlock).transpose()?;
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
    if status == OrderStatus::Paying {
        return Err(ApiError::conflict("already_submitted", "a payment for this order is being sent"));
    }
    if !can_quote(status) {
        return Err(ApiError::conflict("order_not_payable", format!("order is {status}")));
    }
    let now = now();
    if now >= order.expires_at {
        return Err(ApiError::conflict("order_expired", "the time to pay this order has run out"));
    }

    let quoting = Quoting {
        order_bytes: order_id.parse().map_err(ApiError::internal)?,
        order_id,
        payer,
        amount: order.amount.parse().map_err(ApiError::internal)?,
        merchant: order.payout_address.parse().map_err(ApiError::internal)?,
        order_expires_at: order.expires_at,
        now,
    };
    let paid_with = body.token.as_deref().filter(|symbol| *symbol != USDC);
    if let Some(symbol) = paid_with {
        let Some(hashlock) = hashlock else {
            return Err(ApiError::bad_request("bad_token", "a token is paid from a customer chain: send hashlock and chainId"));
        };
        if body.legs.is_some() {
            return Err(ApiError::bad_request("bad_token", "a split is paid in USDC"));
        }
        let chain = source_chain(&state, body.chain_id)?;
        let token = chain
            .tokens
            .get(symbol)
            .filter(|token| token.price_id.is_some())
            .ok_or_else(|| ApiError::bad_request("bad_token", format!("{symbol} is not a token customers can pay with on chain {}", chain.chain_id)))?;
        let price_id = token.price_id.as_deref().unwrap_or_default();
        let price = state.prices.micro_usd(price_id).await.map_err(|error| {
            ApiError::conflict("price_unavailable", format!("no price for {symbol} right now ({error}); try again"))
        })?;
        let priced = token_lock(&state, &quoting, hashlock, chain, symbol, token, price)?;
        return store_and_reply(&state, &quoting, priced).await;
    }
    let priced = match (hashlock, body.legs) {
        (None, None) => direct(&state, &quoting)?,
        (None, Some(_)) => return Err(ApiError::bad_request("bad_legs", "a split needs a hashlock")),
        (Some(hashlock), None) => crosschain(&state, &quoting, hashlock, body.chain_id)?,
        (Some(_), Some(_)) if body.chain_id.is_some() => {
            return Err(ApiError::bad_request("bad_legs", "send either chainId or legs, not both"));
        }
        (Some(hashlock), Some(legs)) => split(&state, &quoting, hashlock, &legs)?,
    };
    store_and_reply(&state, &quoting, priced).await
}

/// Path A: a USDT note for DirectPay on the shop chain.
fn direct(state: &AppState, quoting: &Quoting) -> Result<Priced, ApiError> {
    let chain = state.chains.shop_chain();
    let usdt = token(chain, USDT)?;
    let direct_pay =
        chain.contracts.direct_pay.ok_or_else(|| ApiError::internal("chains.json has no DirectPay address"))?;
    let note = Note {
        from: quoting.payer,
        to: direct_pay,
        value: quoting.amount,
        valid_after: quoting.now - CLOCK_SLACK_SECONDS,
        valid_before: quoting.now + NOTE_LIFETIME_SECONDS,
        nonce: direct_pay::payment_id_for(
            chain.chain_id,
            direct_pay,
            quoting.order_bytes,
            quoting.merchant,
            quoting.amount,
            quoting.payer,
        ),
    };
    let typed_data = direct_pay::typed_data(usdt, chain.chain_id, &note)?;
    let total = quoting.amount.to_decimal(usdt.decimals);
    Ok(Priced {
        path: PayPath::Direct,
        notes: vec![Planned {
            to_sign: ToSign {
                chain_id: chain.chain_id,
                token: USDT.into(),
                amount: total.clone(),
                typed_data: typed_data.clone(),
                lock_id: None,
                signing: "note",
                token_address: None,
                decimals: None,
                calibur: None,
            },
            typed_data,
            note,
            leg: None,
        }],
        total,
        fee: Amount::default().to_decimal(usdt.decimals),
        hashlock: None,
    })
}

/// Path B: a USDC note that opens a SourceLock for Bob, for the order plus his fee.
fn crosschain(state: &AppState, quoting: &Quoting, hashlock: B256, chain_id: Option<u64>) -> Result<Priced, ApiError> {
    let provider = provider(state)?;
    let chain = source_chain(state, chain_id)?;
    let amount = total_with_fee(state, quoting)?;
    let expiry = lock_expiry(state, quoting)?;
    let mut planned = plan_lock(quoting, hashlock, provider, chain, amount, expiry)?;
    // A single-lock quote keeps its note in `quotes.typed_data`.
    if let Some(leg) = planned.leg.as_mut() {
        leg.typed_data = None;
    }
    Ok(Priced {
        path: PayPath::CrossChain,
        notes: vec![planned],
        total: amount.to_decimal(USDC_DECIMALS),
        fee: Amount(amount.0 - quoting.amount.0).to_decimal(USDC_DECIMALS),
        hashlock: Some(hashlock),
    })
}

/// Path B with any listed ERC-20: a Calibur batch (approve + lock) that opens a SourceLock for Bob
/// holding enough of the token, at its market price, to cover the order plus his fee.
fn token_lock(
    state: &AppState,
    quoting: &Quoting,
    hashlock: B256,
    chain: &Chain,
    symbol: &str,
    token: &Token,
    price_micro_usd: u64,
) -> Result<Priced, ApiError> {
    let provider = provider(state)?;
    let source_lock = chain.contracts.source_lock.ok_or_else(|| ApiError::internal("checked by source_chain"))?;
    let calibur_address = chain.contracts.calibur.ok_or_else(|| {
        ApiError::conflict("bad_token", format!("chain {} cannot take tokens without EIP-3009 yet", chain.chain_id))
    })?;
    let usd = total_with_fee(state, quoting)?;
    let amount = token_amount(usd, token.decimals, price_micro_usd)
        .ok_or_else(|| ApiError::bad_request("bad_amount", "the order amount is too large"))?;
    let params = SourceLockParams {
        order_id: quoting.order_bytes,
        receiver: provider,
        min_usdc_out: usd,
        hashlock,
        expiry: lock_expiry(state, quoting)?,
        out_token: Address::ZERO,
    };
    let lock_id = source_lock_id(chain.chain_id, source_lock, &params, quoting.payer, token.address, amount);
    let nonce = calibur::nonce_for(lock_id);
    let deadline = quoting.now + LOCK_NOTE_LIFETIME_SECONDS;
    let deadline_u64 = u64::try_from(deadline).map_err(ApiError::internal)?;
    let batch = calibur::signed_batch(calibur::lock_calls(source_lock, &params, token.address, amount), nonce, deadline_u64);
    let typed_data = calibur::typed_data(&batch, chain.chain_id, quoting.payer, calibur_address);
    // Stored like a note: from the customer to SourceLock, under the lock id.
    let note = Note {
        from: quoting.payer,
        to: source_lock,
        value: amount,
        valid_after: quoting.now - CLOCK_SLACK_SECONDS,
        valid_before: deadline,
        nonce: lock_id,
    };
    let total = amount.to_decimal(token.decimals);
    Ok(Priced {
        path: PayPath::CrossChain,
        notes: vec![Planned {
            to_sign: ToSign {
                chain_id: chain.chain_id,
                token: symbol.to_owned(),
                amount: total.clone(),
                typed_data: typed_data.clone(),
                lock_id: Some(lock_id),
                signing: "batch",
                token_address: Some(token.address),
                decimals: Some(token.decimals),
                calibur: Some(calibur_address),
            },
            leg: Some(PlannedLeg {
                chain_id: chain.chain_id,
                token: token.address,
                amount,
                params,
                lock_id,
                typed_data: None,
                batch: Some(PlannedBatch { nonce, deadline: deadline_u64 }),
            }),
            typed_data,
            note,
        }],
        total,
        fee: Amount(usd.0 - quoting.amount.0).to_decimal(USDC_DECIMALS),
        hashlock: Some(hashlock),
    })
}

/// How much of a token is worth `usd` (6 decimals) at `price_micro_usd` per whole token, plus the
/// quote buffer, rounded up: `ceil(usd × 10^decimals × 10000 / (price × (10000 − buffer)))`.
pub fn token_amount(usd: Amount, decimals: u8, price_micro_usd: u64) -> Option<Amount> {
    if price_micro_usd == 0 {
        return None;
    }
    let scale = U256::from(10u64).checked_pow(U256::from(decimals))?;
    let top = usd.0.checked_mul(scale)?.checked_mul(U256::from(10_000u64))?;
    let bottom = U256::from(price_micro_usd).checked_mul(U256::from(10_000 - QUOTE_BUFFER_BPS))?;
    let amount = top.div_ceil(bottom);
    (amount > U256::ZERO).then_some(Amount(amount))
}

/// Path C: one USDC note per chain, each opening a SourceLock for Bob under the same hashlock and
/// expiry. Together they hold the order plus Bob's fee.
fn split(state: &AppState, quoting: &Quoting, hashlock: B256, legs: &[LegRequest]) -> Result<Priced, ApiError> {
    let provider = provider(state)?;
    let bad = |message: String| ApiError::bad_request("bad_legs", message);
    if legs.len() < 2 {
        return Err(bad("a split needs at least two legs; for one chain send chainId".into()));
    }
    let total = total_with_fee(state, quoting)?;
    let mut chains = Vec::with_capacity(legs.len());
    let mut given = alloy::primitives::U256::ZERO;
    let mut open_leg = None;
    for (index, leg) in legs.iter().enumerate() {
        let chain = source_chain(state, Some(leg.chain_id))?;
        if chains.iter().any(|(c, _): &(&Chain, _)| c.chain_id == chain.chain_id) {
            return Err(bad(format!("chain {} is listed twice", chain.chain_id)));
        }
        let amount = match &leg.amount {
            None if open_leg.is_some() => return Err(bad("only one leg may leave out its amount".into())),
            None => {
                open_leg = Some(index);
                None
            }
            Some(text) => {
                let amount = Amount::from_decimal(text, USDC_DECIMALS)
                    .map_err(|_| bad(format!("leg amount {text:?} is not a USDC amount")))?;
                if amount.0 < alloy::primitives::U256::from(MIN_SPLIT_LEG) {
                    return Err(bad("every leg must lock at least 0.1 USDC".into()));
                }
                given = given.checked_add(amount.0).ok_or_else(|| bad("leg amounts are too large".into()))?;
                Some(amount)
            }
        };
        chains.push((chain, amount));
    }
    let short = |given| {
        bad(format!(
            "the legs add up to {} USDC but the order plus Bob's fee is {}",
            Amount(given).to_decimal(USDC_DECIMALS),
            total.to_decimal(USDC_DECIMALS)
        ))
    };
    match open_leg {
        Some(index) => {
            let rest = total
                .0
                .checked_sub(given)
                .filter(|rest| *rest >= alloy::primitives::U256::from(MIN_SPLIT_LEG))
                .ok_or_else(|| short(given))?;
            chains[index].1 = Some(Amount(rest));
        }
        None if given != total.0 => return Err(short(given)),
        None => {}
    }

    let expiry = lock_expiry(state, quoting)?;
    let notes = chains
        .into_iter()
        .map(|(chain, amount)| plan_lock(quoting, hashlock, provider, chain, amount.unwrap_or_default(), expiry))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Priced {
        path: PayPath::Split,
        notes,
        total: total.to_decimal(USDC_DECIMALS),
        fee: Amount(total.0 - quoting.amount.0).to_decimal(USDC_DECIMALS),
        hashlock: Some(hashlock),
    })
}

/// A note that opens a SourceLock on `chain` for Bob, holding `amount` USDC until `expiry`.
fn plan_lock(
    quoting: &Quoting,
    hashlock: B256,
    provider: Address,
    chain: &Chain,
    amount: Amount,
    expiry: u64,
) -> Result<Planned, ApiError> {
    let source_lock = chain.contracts.source_lock.ok_or_else(|| ApiError::internal("checked by source_chain"))?;
    let usdc = token(chain, USDC)?;
    let params = SourceLockParams {
        order_id: quoting.order_bytes,
        receiver: provider,
        min_usdc_out: amount,
        hashlock,
        expiry,
        out_token: Address::ZERO,
    };
    let lock_id = source_lock_id(chain.chain_id, source_lock, &params, quoting.payer, usdc.address, amount);
    let note = Note {
        from: quoting.payer,
        to: source_lock,
        value: amount,
        valid_after: quoting.now - CLOCK_SLACK_SECONDS,
        valid_before: quoting.now + LOCK_NOTE_LIFETIME_SECONDS,
        nonce: lock_id,
    };
    let typed_data = direct_pay::typed_data(usdc, chain.chain_id, &note)?;
    Ok(Planned {
        to_sign: ToSign {
            chain_id: chain.chain_id,
            token: USDC.into(),
            amount: amount.to_decimal(usdc.decimals),
            typed_data: typed_data.clone(),
            lock_id: Some(lock_id),
            signing: "note",
            token_address: None,
            decimals: None,
            calibur: None,
        },
        leg: Some(PlannedLeg {
            chain_id: chain.chain_id,
            token: usdc.address,
            amount,
            params,
            lock_id,
            typed_data: Some(typed_data.clone()),
            batch: None,
        }),
        typed_data,
        note,
    })
}

fn provider(state: &AppState) -> Result<Address, ApiError> {
    state.provider.ok_or_else(|| {
        ApiError::conflict("crosschain_unavailable", "paying from another chain is not set up on this server")
    })
}

/// The USDC the customer locks in all: the order plus Bob's fee, rounded up.
fn total_with_fee(state: &AppState, quoting: &Quoting) -> Result<Amount, ApiError> {
    with_provider_fee(quoting.amount, state.chains.provider_fee_bps)
        .ok_or_else(|| ApiError::bad_request("bad_amount", "the order amount is too large"))
}

fn lock_expiry(state: &AppState, quoting: &Quoting) -> Result<u64, ApiError> {
    let lock_seconds = i64::try_from(state.chains.timeouts_seconds.source_lock).map_err(ApiError::internal)?;
    u64::try_from(quoting.now + lock_seconds).map_err(ApiError::internal)
}

async fn store_and_reply(state: &AppState, quoting: &Quoting, priced: Priced) -> Result<Json<QuoteView>, ApiError> {
    let quote_id = format!("q_{}", hex::encode(&random::bytes32()[..16]));
    let timeout = i64::try_from(state.chains.timeouts_seconds.quote).map_err(ApiError::internal)?;
    let expires_at = (quoting.now + timeout).min(quoting.order_expires_at);
    let planned_legs: Vec<&PlannedLeg> = priced.notes.iter().filter_map(|n| n.leg.as_ref()).collect();
    let leg = match planned_legs.as_slice() {
        [] => None,
        [one] if priced.path != PayPath::Split => Some(serde_json::to_value(one)),
        many => Some(serde_json::to_value(many)),
    }
    .transpose()
    .map_err(ApiError::internal)?;
    let first = &priced.notes[0];

    // Only from a status that may be quoted, checked again inside the update so two quotes racing
    // on one order cannot both pass, and a quote cannot slip in while a payment is being sent.
    let from_statuses: Vec<&str> = OrderStatus::ALL
        .iter()
        .filter(|status| can_quote(**status))
        .map(|status| status.as_str())
        .collect();
    let mut tx = state.pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE orders SET status = 'quoted', path = $2, payer = $3, hashlock = $4
         WHERE id = $1 AND status = ANY($5)",
    )
    .bind(&quoting.order_id)
    .bind(priced.path.as_str())
    .bind(format!("{:#x}", quoting.payer))
    .bind(priced.hashlock.map(|hashlock| hashlock.to_string()))
    .bind(&from_statuses)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::conflict("order_not_payable", "the order changed while quoting; try again"));
    }
    sqlx::query(
        "INSERT INTO quotes (id, order_id, path, payer, typed_data, nonce, valid_after, valid_before, expires_at,
                             created_at, leg)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(&quote_id)
    .bind(&quoting.order_id)
    .bind(priced.path.as_str())
    .bind(format!("{:#x}", quoting.payer))
    .bind(&first.typed_data)
    .bind(first.note.nonce.to_string())
    .bind(first.note.valid_after)
    .bind(first.note.valid_before)
    .bind(expires_at)
    .bind(quoting.now)
    .bind(leg)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(QuoteView {
        quote_id,
        path: priced.path,
        expires_at,
        total: priced.total,
        fee: priced.fee,
        to_sign: priced.notes.into_iter().map(|n| n.to_sign).collect(),
    }))
}

/// The customer chain to lock on: the one asked for, or the only one with a SourceLock.
fn source_chain(state: &AppState, chain_id: Option<u64>) -> Result<&Chain, ApiError> {
    let usable = |chain: &&Chain| chain.contracts.source_lock.is_some() && chain.tokens.contains_key(USDC);
    match chain_id {
        Some(id) => state.chains.customer_chains().filter(usable).find(|chain| chain.chain_id == id).ok_or_else(|| {
            ApiError::bad_request("bad_chain", format!("chain {id} is not a chain customers can pay from yet"))
        }),
        None => {
            let mut chains = state.chains.customer_chains().filter(usable);
            match (chains.next(), chains.next()) {
                (Some(chain), None) => Ok(chain),
                (None, _) => Err(ApiError::conflict("crosschain_unavailable", "no customer chain has a SourceLock yet")),
                (Some(_), Some(_)) => Err(ApiError::bad_request("bad_chain", "chainId is required: several chains are open")),
            }
        }
    }
}

fn token<'a>(chain: &'a Chain, symbol: &str) -> Result<&'a Token, ApiError> {
    chain.tokens.get(symbol).ok_or_else(|| ApiError::internal(format!("chains.json has no {symbol} on chain {}", chain.chain_id)))
}

fn parse_hashlock(text: &str) -> Result<B256, ApiError> {
    let bad = || ApiError::bad_request("bad_hashlock", "hashlock must be 32 bytes of 0x hex, not zero");
    let hashlock: B256 = text.parse().map_err(|_| bad())?;
    if hashlock == B256::ZERO { Err(bad()) } else { Ok(hashlock) }
}

/// `paying -> quoted` is allowed, but only for the Relayer when a payment fails. A new quote while
/// a payment is being sent would let a second payer sign for the same order.
fn can_quote(status: OrderStatus) -> bool {
    status != OrderStatus::Paying && status.can_move(OrderStatus::Quoted)
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

    const HASHLOCK: &str = "0x4242424242424242424242424242424242424242424242424242424242424242";

    #[sqlx::test]
    async fn a_hashlock_makes_a_usdc_note_that_opens_a_source_lock_for_bob(pool: PgPool) {
        use common::locks::{SourceLockParams, source_lock_id};

        let test = test_app(pool).await;
        let id = new_order(&test).await;
        let (status, reply) = call(&test.app, quote(&id, json!({ "payer": PAYER, "hashlock": HASHLOCK, "chainId": 84532 }))).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!(reply["path"], "crosschain");
        assert_eq!((reply["total"].as_str(), reply["fee"].as_str()), (Some("12.036000"), Some("0.036000")));

        let chains = common::chains::Chains::from_json(include_str!("../../../../shared/chains.json")).unwrap();
        let base = chains.by_id(84532).unwrap();
        let (source_lock, usdc) = (base.contracts.source_lock.unwrap(), base.tokens["USDC"].address);
        let note = &reply["toSign"][0];
        assert_eq!((note["chainId"].as_u64(), note["token"].as_str()), (Some(84532), Some("USDC")));
        let typed = &note["typedData"];
        assert_eq!(
            typed["domain"],
            json!({ "name": "USDC", "version": "2", "chainId": 84532, "verifyingContract": format!("{usdc:#x}") })
        );
        let message = &typed["message"];
        assert_eq!(message["to"], format!("{source_lock:#x}"));
        assert_eq!(message["value"], "12036000");
        let after: u64 = message["validAfter"].as_str().unwrap().parse().unwrap();
        let before: u64 = message["validBefore"].as_str().unwrap().parse().unwrap();
        assert_eq!(before - after, 360);

        // The nonce is the lock id SourceLock will compute: Bob, the hashlock, 20 minutes.
        let params = SourceLockParams {
            order_id: id.parse().unwrap(),
            receiver: crate::routes::test_support::PROVIDER,
            min_usdc_out: Amount::from(12_036_000u64),
            hashlock: HASHLOCK.parse().unwrap(),
            expiry: after + 60 + 1200,
            out_token: Address::ZERO,
        };
        let lock_id = source_lock_id(84532, source_lock, &params, PAYER.parse().unwrap(), usdc, Amount::from(12_036_000u64));
        assert_eq!(message["nonce"], lock_id.to_string());
        assert_eq!(note["lockId"], lock_id.to_string());
        assert!(before + 15 * 60 <= params.expiry, "SourceLock's rule: the note ends 15 minutes before the lock");

        let (path, hashlock, leg): (String, String, Value) = sqlx::query_as(
            "SELECT o.path, o.hashlock, q.leg FROM orders o JOIN quotes q ON q.order_id = o.id WHERE o.id = $1",
        )
        .bind(&id)
        .fetch_one(&test.pool)
        .await
        .unwrap();
        assert_eq!((path.as_str(), hashlock.as_str()), ("crosschain", HASHLOCK));
        assert_eq!(leg["lockId"], lock_id.to_string());
        assert_eq!(leg["params"]["receiver"], format!("{:#x}", crate::routes::test_support::PROVIDER));
    }

    #[test]
    fn a_token_amount_covers_the_order_at_the_price_plus_the_buffer() {
        // 12.036 USD of WBTC at 100,000 USD: 0.00012036 WBTC, /0.97 and rounded up.
        let amount = super::token_amount(Amount::from(12_036_000u64), 8, 100_000_000_000).unwrap();
        assert_eq!(amount, Amount::from(12_409u64));
        // Worth at least 12.036 USD at the Matchmaker's 1% discount.
        let value = amount.0 * alloy::primitives::U256::from(100_000_000_000u64) * alloy::primitives::U256::from(9_900u64)
            / (alloy::primitives::U256::from(100_000_000u64) * alloy::primitives::U256::from(10_000u64));
        assert!(value >= alloy::primitives::U256::from(12_036_000u64), "{value}");
        // WETH at 4,000 USD, 18 decimals.
        let weth = super::token_amount(Amount::from(12_036_000u64), 18, 4_000_000_000).unwrap();
        assert_eq!(weth, Amount::from(3_102_061_855_670_104u64));
        assert_eq!(super::token_amount(Amount::from(1u64), 8, 0), None);
    }

    #[sqlx::test]
    async fn a_wbtc_quote_is_a_calibur_batch_that_approves_and_locks_for_bob(pool: PgPool) {
        use common::calibur;
        use common::locks::{SourceLockParams, source_lock_id};

        let test = test_app(pool).await;
        let id = new_order(&test).await;
        let body = json!({ "payer": PAYER, "hashlock": HASHLOCK, "chainId": 11155111, "token": "WBTC" });
        let (status, reply) = call(&test.app, quote(&id, body)).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!((reply["path"].as_str(), reply["total"].as_str(), reply["fee"].as_str()), (Some("crosschain"), Some("0.00012409"), Some("0.036000")));

        let chains = common::chains::Chains::from_json(include_str!("../../../../shared/chains.json")).unwrap();
        let eth = chains.by_id(11155111).unwrap();
        let (source_lock, wbtc, calibur_address) = (eth.contracts.source_lock.unwrap(), eth.tokens["WBTC"].address, eth.contracts.calibur.unwrap());
        let note = &reply["toSign"][0];
        assert_eq!((note["signing"].as_str(), note["token"].as_str(), note["amount"].as_str()), (Some("batch"), Some("WBTC"), Some("0.00012409")));
        assert_eq!(note["calibur"], format!("{calibur_address:#x}"));

        // Exactly what the Gas Payer will rebuild: approve then lock, for Bob, under the lock id.
        let typed = &note["typedData"];
        let expiry: u64 = typed["message"]["deadline"].as_str().unwrap().parse::<u64>().unwrap() - 300 + 1200;
        let params = SourceLockParams {
            order_id: id.parse().unwrap(),
            receiver: crate::routes::test_support::PROVIDER,
            min_usdc_out: Amount::from(12_036_000u64),
            hashlock: HASHLOCK.parse().unwrap(),
            expiry,
            out_token: Address::ZERO,
        };
        let payer: Address = PAYER.parse().unwrap();
        let lock_id = source_lock_id(11155111, source_lock, &params, payer, wbtc, Amount::from(12_409u64));
        assert_eq!(note["lockId"], lock_id.to_string());
        let batch = calibur::signed_batch(
            calibur::lock_calls(source_lock, &params, wbtc, Amount::from(12_409u64)),
            calibur::nonce_for(lock_id),
            expiry - 1200 + 300,
        );
        assert_eq!(typed, &calibur::typed_data(&batch, 11155111, payer, calibur_address));
    }

    #[sqlx::test]
    async fn unknown_or_unpriced_tokens_are_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let id = new_order(&test).await;
        for (token, chain) in [("WBTC", 84532), ("EURC", 11155111), ("DOGE", 11155111)] {
            let body = json!({ "payer": PAYER, "hashlock": HASHLOCK, "chainId": chain, "token": token });
            let (status, reply) = call(&test.app, quote(&id, body)).await;
            assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_token")), "{token} on {chain}");
        }
        let (status, _) = call(&test.app, quote(&id, json!({ "payer": PAYER, "token": "WBTC" }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test]
    async fn ethereum_sepolia_gets_a_note_for_its_own_source_lock(pool: PgPool) {
        let test = test_app(pool).await;
        let id = new_order(&test).await;
        let (status, reply) =
            call(&test.app, quote(&id, json!({ "payer": PAYER, "hashlock": HASHLOCK, "chainId": 11155111 }))).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        let chains = common::chains::Chains::from_json(include_str!("../../../../shared/chains.json")).unwrap();
        let ethereum = chains.by_id(11155111).unwrap();
        let typed = &reply["toSign"][0]["typedData"];
        assert_eq!(typed["domain"]["chainId"], 11155111);
        assert_eq!(typed["domain"]["verifyingContract"], format!("{:#x}", ethereum.tokens["USDC"].address));
        assert_eq!(typed["message"]["to"], format!("{:#x}", ethereum.contracts.source_lock.unwrap()));
    }

    #[sqlx::test]
    async fn bad_hashlocks_and_chains_are_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let id = new_order(&test).await;
        let zero = format!("0x{}", "00".repeat(32));
        for (body, code) in [
            (json!({ "payer": PAYER, "hashlock": "0x1234" }), "bad_hashlock"),
            (json!({ "payer": PAYER, "hashlock": zero }), "bad_hashlock"),
            (json!({ "payer": PAYER, "hashlock": HASHLOCK, "chainId": 1874 }), "bad_chain"),
            (json!({ "payer": PAYER, "hashlock": HASHLOCK, "chainId": 1 }), "bad_chain"),
            (json!({ "payer": PAYER, "hashlock": HASHLOCK }), "bad_chain"),
        ] {
            let (status, reply) = call(&test.app, quote(&id, body.clone())).await;
            assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{body}");
        }
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

    fn split_body(legs: Value) -> Value {
        json!({ "payer": PAYER, "hashlock": B256::repeat_byte(0x77), "legs": legs })
    }

    #[sqlx::test]
    async fn a_split_quote_is_one_lock_note_per_chain_adding_up_to_the_order_plus_fee(pool: PgPool) {
        let test = test_app(pool).await;
        let id = new_order(&test).await;
        let body = split_body(json!([{ "chainId": 84532, "amount": "6" }, { "chainId": 11155111 }]));
        let (status, reply) = call(&test.app, quote(&id, body)).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!(reply["path"], "split");
        assert_eq!((reply["total"].as_str(), reply["fee"].as_str()), (Some("12.036000"), Some("0.036000")));
        let notes = reply["toSign"].as_array().unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0]["chainId"].as_u64(), notes[0]["amount"].as_str()), (Some(84532), Some("6.000000")));
        assert_eq!((notes[1]["chainId"].as_u64(), notes[1]["amount"].as_str()), (Some(11155111), Some("6.036000")));
        assert_ne!(notes[0]["lockId"], notes[1]["lockId"]);
        for note in notes {
            let typed = &note["typedData"];
            assert_eq!(typed["domain"]["chainId"], note["chainId"]);
            assert_eq!(typed["message"]["nonce"], note["lockId"]);
        }
        assert_eq!(notes[0]["typedData"]["message"]["validBefore"], notes[1]["typedData"]["message"]["validBefore"]);
    }

    #[sqlx::test]
    async fn a_split_quote_refuses_legs_that_do_not_make_one_payment(pool: PgPool) {
        let test = test_app(pool).await;
        let id = new_order(&test).await;
        let refused = [
            split_body(json!([{ "chainId": 84532 }])),
            split_body(json!([{ "chainId": 84532, "amount": "6" }, { "chainId": 84532 }])),
            split_body(json!([{ "chainId": 84532, "amount": "6" }, { "chainId": 11155111, "amount": "6" }])),
            split_body(json!([{ "chainId": 84532, "amount": "6" }, { "chainId": 11155111, "amount": "7" }])),
            split_body(json!([{ "chainId": 84532 }, { "chainId": 11155111 }])),
            split_body(json!([{ "chainId": 84532, "amount": "12.036" }, { "chainId": 11155111 }])),
            split_body(json!([{ "chainId": 84532, "amount": "0" }, { "chainId": 11155111 }])),
            split_body(json!([{ "chainId": 84532, "amount": "0.099999" }, { "chainId": 11155111 }])),
            split_body(json!([{ "chainId": 84532, "amount": "11.99" }, { "chainId": 11155111 }])),
            split_body(json!([{ "chainId": 84532, "amount": "six" }, { "chainId": 11155111 }])),
            json!({ "payer": PAYER, "legs": [{ "chainId": 84532, "amount": "6" }, { "chainId": 11155111 }] }),
            json!({ "payer": PAYER, "hashlock": B256::repeat_byte(0x77), "chainId": 84532,
                    "legs": [{ "chainId": 84532, "amount": "6" }, { "chainId": 11155111 }] }),
        ];
        for body in refused {
            let (status, reply) = call(&test.app, quote(&id, body.clone())).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body} -> {reply}");
        }
        let (status, reply) = call(&test.app, quote(&id, split_body(json!([{ "chainId": 1874, "amount": "6" }, { "chainId": 84532 }])))).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_chain")));
    }
}
