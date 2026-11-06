//! `POST /v1/orders/{id}/quote`: the pay page asks what the customer must sign.
//!
//! - `{payer}`: Path A, USDT on Whitechain through DirectPay.
//! - `{payer, hashlock, chainId?}`: the cross-chain path. The customer locks USDC on `chainId`
//!   (default: the one customer chain with a SourceLock) for Bob, under the hashlock of a secret
//!   only the pay page knows. The pay page picks the path from balances it reads itself; the
//!   Order Desk connects to no chain, and the Relayer checks the balance before it spends gas.

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
    /// One note per chain the customer pays from: exactly one on Paths A and B.
    to_sign: Vec<ToSign>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToSign {
    chain_id: u64,
    token: &'static str,
    amount: String,
    typed_data: Value,
    /// Cross-chain: the source lock the note opens (also its nonce).
    #[serde(skip_serializing_if = "Option::is_none")]
    lock_id: Option<B256>,
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

/// One quote, ready to store and return.
struct Priced {
    path: PayPath,
    note: Note,
    typed_data: Value,
    to_sign: ToSign,
    total: String,
    fee: String,
    hashlock: Option<B256>,
    leg: Option<PlannedLeg>,
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
    let priced = match hashlock {
        None => direct(&state, &quoting)?,
        Some(hashlock) => crosschain(&state, &quoting, hashlock, body.chain_id)?,
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
    let typed_data = direct_pay::typed_data(usdt, chain.chain_id, &note);
    let total = quoting.amount.to_decimal(usdt.decimals);
    Ok(Priced {
        path: PayPath::Direct,
        to_sign: ToSign {
            chain_id: chain.chain_id,
            token: USDT,
            amount: total.clone(),
            typed_data: typed_data.clone(),
            lock_id: None,
        },
        typed_data,
        note,
        total,
        fee: Amount::default().to_decimal(usdt.decimals),
        hashlock: None,
        leg: None,
    })
}

/// Path B: a USDC note that opens a SourceLock for Bob, for the order plus his fee.
fn crosschain(state: &AppState, quoting: &Quoting, hashlock: B256, chain_id: Option<u64>) -> Result<Priced, ApiError> {
    let provider = state.provider.ok_or_else(|| {
        ApiError::conflict("crosschain_unavailable", "paying from another chain is not set up on this server")
    })?;
    let chain = source_chain(state, chain_id)?;
    let source_lock = chain.contracts.source_lock.ok_or_else(|| ApiError::internal("checked by source_chain"))?;
    let usdc = token(chain, USDC)?;
    let fee_bps = state.chains.provider_fee_bps;
    let amount = with_provider_fee(quoting.amount, fee_bps)
        .ok_or_else(|| ApiError::bad_request("bad_amount", "the order amount is too large"))?;
    let lock_seconds = i64::try_from(state.chains.timeouts_seconds.source_lock).map_err(ApiError::internal)?;
    let params = SourceLockParams {
        order_id: quoting.order_bytes,
        receiver: provider,
        min_usdc_out: amount,
        hashlock,
        expiry: u64::try_from(quoting.now + lock_seconds).map_err(ApiError::internal)?,
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
    let typed_data = direct_pay::typed_data(usdc, chain.chain_id, &note);
    let total = amount.to_decimal(usdc.decimals);
    let fee = Amount(amount.0 - quoting.amount.0).to_decimal(usdc.decimals);
    Ok(Priced {
        path: PayPath::CrossChain,
        to_sign: ToSign {
            chain_id: chain.chain_id,
            token: USDC,
            amount: total.clone(),
            typed_data: typed_data.clone(),
            lock_id: Some(lock_id),
        },
        typed_data,
        note,
        total,
        fee,
        hashlock: Some(hashlock),
        leg: Some(PlannedLeg { chain_id: chain.chain_id, token: usdc.address, amount, params, lock_id }),
    })
}

async fn store_and_reply(state: &AppState, quoting: &Quoting, priced: Priced) -> Result<Json<QuoteView>, ApiError> {
    let quote_id = format!("q_{}", hex::encode(&random::bytes32()[..16]));
    let timeout = i64::try_from(state.chains.timeouts_seconds.quote).map_err(ApiError::internal)?;
    let expires_at = (quoting.now + timeout).min(quoting.order_expires_at);
    let leg = priced.leg.as_ref().map(serde_json::to_value).transpose().map_err(ApiError::internal)?;

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
    .bind(&priced.typed_data)
    .bind(priced.note.nonce.to_string())
    .bind(priced.note.valid_after)
    .bind(priced.note.valid_before)
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
        to_sign: vec![priced.to_sign],
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
        let (status, reply) = call(&test.app, quote(&id, json!({ "payer": PAYER, "hashlock": HASHLOCK }))).await;
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

    #[sqlx::test]
    async fn bad_hashlocks_and_chains_are_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let id = new_order(&test).await;
        let zero = format!("0x{}", "00".repeat(32));
        for (body, code) in [
            (json!({ "payer": PAYER, "hashlock": "0x1234" }), "bad_hashlock"),
            (json!({ "payer": PAYER, "hashlock": zero }), "bad_hashlock"),
            (json!({ "payer": PAYER, "hashlock": HASHLOCK, "chainId": 1874 }), "bad_chain"),
            (json!({ "payer": PAYER, "hashlock": HASHLOCK, "chainId": 11155111 }), "bad_chain"),
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
}
