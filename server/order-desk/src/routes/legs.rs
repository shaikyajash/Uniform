//! `POST /v1/orders/{id}/legs`: the pay page sends the customer's signatures on a cross-chain or
//! split quote, one per note in `toSign`, in that order (`signature` alone for a single note).
//!
//! The Order Desk checks every signature, records the legs they open and writes a `source_lock`
//! job per leg for the Relayer, which locks the customer's USDC for Bob and pays the gas. The order
//! becomes `paying`; the `Locked` events move it on once every leg is locked.

use alloy::primitives::{Address, B256};
use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
};
use common::{calibur::Batch, jobs::Job, status::OrderStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AppState, direct::recover_check, normalize_order_id, now, quote::planned_legs};
use crate::error::ApiError;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegsRequest {
    quote_id: String,
    /// 65 bytes (r, s, v) as 0x hex, from the wallet's `signTypedData` on the quote's one note.
    signature: Option<String>,
    /// The same, one per note in the quote's `toSign`, in that order.
    signatures: Option<Vec<String>>,
    /// For a `batch` note when the wallet is not delegated to Calibur yet: its EIP-7702
    /// authorization (`useSign7702Authorization`), sent with the lock.
    authorization: Option<common::calibur::Authorization>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegsAccepted {
    status: OrderStatus,
    /// The first new lock job, and the first leg's lock.
    job_id: i64,
    lock_id: B256,
    /// Every leg, in the order of `toSign`. `jobId` is null for a leg whose lock job is already
    /// running or done, so signing the same quote again never sends a lock twice.
    locks: Vec<LockAccepted>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockAccepted {
    chain_id: u64,
    lock_id: B256,
    job_id: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct QuotedLeg {
    payer: String,
    typed_data: Value,
    valid_after: i64,
    valid_before: i64,
    expires_at: i64,
    leg: Option<Value>,
    status: String,
    merchant_id: i64,
}

pub async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<LegsRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<LegsAccepted>), ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let order_id = normalize_order_id(&id).ok_or_else(|| ApiError::not_found("order"))?;

    let quote: QuotedLeg = sqlx::query_as(
        "SELECT q.payer, q.typed_data, q.valid_after, q.valid_before, q.expires_at, q.leg, o.status, o.merchant_id
         FROM quotes q JOIN orders o ON o.id = q.order_id
         WHERE q.id = $1 AND q.order_id = $2 AND q.path IN ('crosschain', 'split')",
    )
    .bind(&body.quote_id)
    .bind(&order_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("quote"))?;

    let status: OrderStatus = quote.status.parse().map_err(ApiError::internal)?;
    if status == OrderStatus::Paying {
        return Err(ApiError::conflict("already_submitted", "this order's payment is already being sent"));
    }
    if !status.can_move(OrderStatus::Paying) {
        return Err(ApiError::conflict("order_not_payable", format!("order is {status}")));
    }
    if now() >= quote.expires_at {
        return Err(ApiError::conflict("quote_expired", "the quote ran out; ask for a new one and sign again"));
    }
    let legs = planned_legs(
        quote.leg.ok_or_else(|| ApiError::internal("cross-chain quote without a leg"))?,
        &quote.typed_data,
    )
    .map_err(ApiError::internal)?;
    let signatures = match (body.signature, body.signatures) {
        (Some(one), None) => vec![one],
        (None, Some(many)) => many,
        _ => return Err(ApiError::bad_request("bad_signature", "send signature, or signatures with one per note")),
    };
    if signatures.len() != legs.len() {
        return Err(ApiError::bad_request(
            "bad_signature",
            format!("this quote has {} notes to sign; got {} signatures", legs.len(), signatures.len()),
        ));
    }
    let payer: Address = quote.payer.parse().map_err(ApiError::internal)?;
    if let Some(authorization) = body.authorization {
        let leg = match legs.as_slice() {
            [leg] if leg.batch.is_some() => leg,
            _ => return Err(ApiError::bad_request("bad_authorization", "an authorization goes only with a token batch")),
        };
        let calibur = state.chains.by_id(leg.chain_id).and_then(|chain| chain.contracts.calibur);
        if Some(authorization.address) != calibur || authorization.chain_id != leg.chain_id {
            return Err(ApiError::bad_request("bad_authorization", "the authorization must delegate to Calibur on this chain"));
        }
    }
    let mut jobs = Vec::with_capacity(legs.len());
    for (leg, signature) in legs.iter().zip(&signatures) {
        let typed_data = leg.typed_data.as_ref().ok_or_else(|| ApiError::internal("planned leg without its note"))?;
        let signature = recover_check(signature, typed_data, payer)?;
        // A batch's time limit is its own deadline (in what was signed), not a note's window.
        let (valid_after, valid_before, batch) = match leg.batch {
            Some(planned) => {
                (0, 0, Some(Batch { nonce: planned.nonce, deadline: planned.deadline, authorization: body.authorization }))
            }
            None => (
                u64::try_from(quote.valid_after).map_err(ApiError::internal)?,
                u64::try_from(quote.valid_before).map_err(ApiError::internal)?,
                None,
            ),
        };
        jobs.push(Job::SourceLock {
            params: leg.params,
            payer,
            token: leg.token,
            amount: leg.amount,
            valid_after,
            valid_before,
            lock_id: leg.lock_id,
            signature,
            batch,
        });
    }
    let hashlock = legs[0].params.hashlock;
    let now = now();

    let mut tx = state.pool.begin().await?;
    // Only from `quoted`, checked again here: a double click cannot make two jobs. The hashlock is
    // the signed quote's, in case a newer quote with another one was asked for in between.
    let moved = sqlx::query("UPDATE orders SET status = 'paying', hashlock = $2 WHERE id = $1 AND status = 'quoted'")
        .bind(&order_id)
        .bind(hashlock.to_string())
        .execute(&mut *tx)
        .await?;
    if moved.rows_affected() == 0 {
        return Err(ApiError::conflict("already_submitted", "this order's payment is already being sent"));
    }
    let mut locks = Vec::with_capacity(legs.len());
    for (leg, job) in legs.iter().zip(&jobs) {
        let chain_id = i64::try_from(leg.chain_id).map_err(ApiError::internal)?;
        // The same quote may be signed again after a lock job failed: the leg is already there.
        sqlx::query(
            "INSERT INTO legs (order_id, quote_id, chain_id, token, amount, payer, lock_id, expiry, created_at)
             VALUES ($1, $2, $3, $4, $5::numeric, $6, $7, $8, $9)
             ON CONFLICT (chain_id, lock_id) DO NOTHING",
        )
        .bind(&order_id)
        .bind(&body.quote_id)
        .bind(chain_id)
        .bind(format!("{:#x}", leg.token))
        .bind(leg.amount.to_string())
        .bind(format!("{payer:#x}"))
        .bind(leg.lock_id.to_string())
        .bind(i64::try_from(leg.params.expiry).map_err(ApiError::internal)?)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        // A leg whose lock job is running or done keeps it; only the others get a new job.
        let busy: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM relayer_jobs WHERE order_id = $1 AND kind = 'source_lock'
                            AND args->>'lockId' = $2 AND status <> 'failed')",
        )
        .bind(&order_id)
        .bind(leg.lock_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        let job_id = if busy {
            None
        } else {
            Some(super::insert_job(&mut tx, &order_id, quote.merchant_id, chain_id, job, now).await?)
        };
        locks.push(LockAccepted { chain_id: leg.chain_id, lock_id: leg.lock_id, job_id });
    }
    // Nothing new to send: every lock is already being opened. The transaction is dropped, so the
    // order stays as it was.
    let Some(job_id) = locks.iter().find_map(|l| l.job_id) else {
        return Err(ApiError::conflict("already_submitted", "every lock of this quote is already being sent"));
    };
    tx.commit().await?;
    let lock_id = locks[0].lock_id;
    Ok((StatusCode::ACCEPTED, Json(LegsAccepted { status: OrderStatus::Paying, job_id, lock_id, locks })))
}

#[cfg(test)]
pub(crate) mod tests {
    use alloy::{
        dyn_abi::TypedData,
        hex,
        signers::{SignerSync, local::PrivateKeySigner},
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use common::jobs::Job;
    use serde_json::{Value, json};
    use sqlx::PgPool;

    use crate::routes::test_support::{TestApp, call, test_app};

    pub const SECRET: &str = "0x0707070707070707070707070707070707070707070707070707070707070707";

    pub fn post(path: String, body: Value) -> Request<Body> {
        Request::post(path).header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap()
    }

    /// A 12 USDT order with a cross-chain quote for `payer` under SECRET's hashlock.
    pub async fn quoted(test: &TestApp, payer: &PrivateKeySigner) -> (String, Value) {
        let (_, created) = test.create_order(json!({ "amount": "12", "reference": "shop-123" })).await;
        let id = created["orderId"].as_str().unwrap().to_owned();
        let hashlock = common::locks::hashlock_of(SECRET.parse().unwrap());
        let body = json!({ "payer": payer.address(), "hashlock": hashlock, "chainId": 84532 });
        let (status, quote) = call(&test.app, post(format!("/v1/orders/{id}/quote"), body)).await;
        assert_eq!(status, StatusCode::OK, "{quote}");
        (id, quote)
    }

    pub fn sign(signer: &PrivateKeySigner, quote: &Value) -> String {
        let typed: TypedData = serde_json::from_value(quote["toSign"][0]["typedData"].clone()).unwrap();
        let signature = signer.sign_hash_sync(&typed.eip712_signing_hash().unwrap()).unwrap();
        format!("0x{}", hex::encode(signature.as_bytes()))
    }

    pub fn submit(id: &str, quote: &Value, signature: &str) -> Request<Body> {
        post(format!("/v1/orders/{id}/legs"), json!({ "quoteId": quote["quoteId"], "signature": signature }))
    }

    #[sqlx::test]
    async fn a_signed_quote_becomes_a_leg_and_one_lock_job(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(&test, &alice).await;

        let (status, reply) = call(&test.app, submit(&id, &quote, &sign(&alice, &quote))).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{reply}");
        assert_eq!(reply["status"], "paying");
        assert_eq!(reply["lockId"], quote["toSign"][0]["lockId"]);

        let (chain_id, amount, lock_id, leg_status): (i64, String, String, String) =
            sqlx::query_as("SELECT chain_id, amount::text, lock_id, status FROM legs WHERE order_id = $1")
                .bind(&id)
                .fetch_one(&test.pool)
                .await
                .unwrap();
        assert_eq!((chain_id, amount.as_str(), leg_status.as_str()), (84532, "12036000", "pending"));
        assert_eq!(lock_id, quote["toSign"][0]["lockId"].as_str().unwrap());

        let (kind, job_chain, args): (String, i64, Value) =
            sqlx::query_as("SELECT kind, chain_id, args FROM relayer_jobs WHERE id = $1")
                .bind(reply["jobId"].as_i64().unwrap())
                .fetch_one(&test.pool)
                .await
                .unwrap();
        assert_eq!((kind.as_str(), job_chain), ("source_lock", 84532));
        let Job::SourceLock { payer, amount, lock_id, params, valid_before, .. } = serde_json::from_value(args).unwrap() else {
            panic!("expected a source_lock job");
        };
        let message = &quote["toSign"][0]["typedData"]["message"];
        assert_eq!(payer, alice.address());
        assert_eq!(amount.to_string(), "12036000");
        assert_eq!(lock_id.to_string(), message["nonce"].as_str().unwrap());
        assert_eq!(valid_before.to_string(), message["validBefore"].as_str().unwrap());
        assert_eq!(params.receiver, crate::routes::test_support::PROVIDER);

        // The pay page and the Matchmaker see the exact lock id to expect.
        let read = Request::get(format!("/v1/orders/{id}")).body(Body::empty()).unwrap();
        let (_, order) = call(&test.app, read).await;
        assert_eq!(order["path"], "crosschain");
        assert_eq!(order["legs"][0]["lockId"], quote["toSign"][0]["lockId"]);
        assert_eq!((order["legs"][0]["chainId"].as_u64(), order["legs"][0]["status"].as_str()), (Some(84532), Some("pending")));
        assert_eq!(order["payment"]["status"], "pending");
        assert_eq!(order["hashlock"], common::locks::hashlock_of(SECRET.parse().unwrap()).to_string());
    }

    /// A 12 USDT order quoted in WBTC on Ethereum Sepolia for `payer`.
    async fn quoted_in_wbtc(test: &TestApp, payer: &PrivateKeySigner) -> (String, Value) {
        let (_, created) = test.create_order(json!({ "amount": "12", "reference": "shop-wbtc" })).await;
        let id = created["orderId"].as_str().unwrap().to_owned();
        let hashlock = common::locks::hashlock_of(SECRET.parse().unwrap());
        let body = json!({ "payer": payer.address(), "hashlock": hashlock, "chainId": 11155111, "token": "WBTC" });
        let (status, quote) = call(&test.app, post(format!("/v1/orders/{id}/quote"), body)).await;
        assert_eq!(status, StatusCode::OK, "{quote}");
        (id, quote)
    }

    fn calibur() -> String {
        let chains = common::chains::Chains::from_json(include_str!("../../../../shared/chains.json")).unwrap();
        format!("{:#x}", chains.by_id(11155111).unwrap().contracts.calibur.unwrap())
    }

    #[sqlx::test]
    async fn a_wbtc_batch_becomes_a_lock_job_with_its_batch_and_authorization(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted_in_wbtc(&test, &alice).await;
        let note = &quote["toSign"][0];
        assert_eq!((note["signing"].as_str(), note["token"].as_str(), note["decimals"].as_u64()), (Some("batch"), Some("WBTC"), Some(8)));
        assert_eq!(note["typedData"]["domain"]["verifyingContract"], format!("{:#x}", alice.address()));

        let authorization = json!({ "chainId": 11155111, "address": calibur(), "nonce": 0, "yParity": 1, "r": "0x01", "s": "0x02" });
        let body = json!({ "quoteId": quote["quoteId"], "signatures": [sign(&alice, &quote)], "authorization": authorization });
        let (status, reply) = call(&test.app, post(format!("/v1/orders/{id}/legs"), body)).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{reply}");

        let (kind, chain_id, args): (String, i64, Value) =
            sqlx::query_as("SELECT kind, chain_id, args FROM relayer_jobs WHERE id = $1")
                .bind(reply["jobId"].as_i64().unwrap())
                .fetch_one(&test.pool)
                .await
                .unwrap();
        assert_eq!((kind.as_str(), chain_id), ("source_lock", 11155111));
        let Job::SourceLock { payer, token, amount, valid_after, valid_before, batch, .. } = serde_json::from_value(args).unwrap() else {
            panic!("expected a source_lock job");
        };
        let batch = batch.expect("a batch lock");
        assert_eq!(payer, alice.address());
        assert_eq!(format!("{token:#x}"), note["tokenAddress"].as_str().unwrap().to_lowercase());
        assert_eq!(amount.to_string(), note["typedData"]["message"]["batchedCall"]["calls"][1]["data"].as_str().map(|_| amount.to_string()).unwrap());
        assert_eq!((valid_after, valid_before), (0, 0));
        assert_eq!(batch.nonce.to_string(), note["typedData"]["message"]["nonce"].as_str().unwrap());
        assert_eq!(batch.deadline.to_string(), note["typedData"]["message"]["deadline"].as_str().unwrap());
        assert_eq!(batch.authorization.map(|a| format!("{:#x}", a.address)), Some(calibur()));
    }

    #[sqlx::test]
    async fn a_wbtc_batch_without_authorization_is_fine_and_a_foreign_one_is_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted_in_wbtc(&test, &alice).await;
        let foreign = json!({ "chainId": 11155111, "address": "0x000000000000000000000000000000000000dead", "nonce": 0, "yParity": 0, "r": "0x01", "s": "0x02" });
        let body = json!({ "quoteId": quote["quoteId"], "signatures": [sign(&alice, &quote)], "authorization": foreign });
        let (status, reply) = call(&test.app, post(format!("/v1/orders/{id}/legs"), body)).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_authorization")));

        let (status, reply) = call(&test.app, submit(&id, &quote, &sign(&alice, &quote))).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{reply}");
        let args: Value = sqlx::query_scalar("SELECT args FROM relayer_jobs").fetch_one(&test.pool).await.unwrap();
        assert_eq!(args["batch"]["authorization"], Value::Null);
        assert!(args["batch"]["nonce"].is_string() || args["batch"]["nonce"].is_number());
    }

    #[sqlx::test]
    async fn an_authorization_on_a_usdc_note_is_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(&test, &alice).await;
        let authorization = json!({ "chainId": 84532, "address": calibur(), "nonce": 0, "yParity": 0, "r": "0x01", "s": "0x02" });
        let body = json!({ "quoteId": quote["quoteId"], "signature": sign(&alice, &quote), "authorization": authorization });
        let (status, reply) = call(&test.app, post(format!("/v1/orders/{id}/legs"), body)).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_authorization")));
    }

    #[sqlx::test]
    async fn wrong_signers_doubles_and_path_a_quotes_are_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let (alice, mallory) = (PrivateKeySigner::random(), PrivateKeySigner::random());
        let (id, quote) = quoted(&test, &alice).await;

        let (status, reply) = call(&test.app, submit(&id, &quote, &sign(&mallory, &quote))).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_signature")));

        let signature = sign(&alice, &quote);
        assert_eq!(call(&test.app, submit(&id, &quote, &signature)).await.0, StatusCode::ACCEPTED);
        let (status, reply) = call(&test.app, submit(&id, &quote, &signature)).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("already_submitted")));
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM relayer_jobs").fetch_one(&test.pool).await.unwrap();
        assert_eq!(jobs, 1);

        // A Path A quote cannot be sent here.
        let (_, created) = test.create_order(json!({ "amount": "5", "reference": "shop-124" })).await;
        let other = created["orderId"].as_str().unwrap().to_owned();
        let (_, direct) = call(&test.app, post(format!("/v1/orders/{other}/quote"), json!({ "payer": alice.address() }))).await;
        assert_eq!(call(&test.app, submit(&other, &direct, &sign(&alice, &direct))).await.0, StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn an_expired_quote_is_refused(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = quoted(&test, &alice).await;
        sqlx::query("UPDATE quotes SET expires_at = 1").execute(&test.pool).await.unwrap();
        let (status, reply) = call(&test.app, submit(&id, &quote, &sign(&alice, &quote))).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("quote_expired")));
    }

    /// A 12 USDT order with a split quote for `payer`: 6 USDC on Base, the rest on Ethereum.
    pub async fn split_quoted(test: &TestApp, payer: &PrivateKeySigner) -> (String, Value) {
        let (_, created) = test.create_order(json!({ "amount": "12", "reference": "shop-123" })).await;
        let id = created["orderId"].as_str().unwrap().to_owned();
        let hashlock = common::locks::hashlock_of(SECRET.parse().unwrap());
        let body = json!({ "payer": payer.address(), "hashlock": hashlock,
                           "legs": [{ "chainId": 84532, "amount": "6" }, { "chainId": 11155111 }] });
        let (status, quote) = call(&test.app, post(format!("/v1/orders/{id}/quote"), body)).await;
        assert_eq!(status, StatusCode::OK, "{quote}");
        (id, quote)
    }

    /// One signature per note, in the quote's order.
    pub fn sign_all(signer: &PrivateKeySigner, quote: &Value) -> Vec<String> {
        quote["toSign"]
            .as_array()
            .unwrap()
            .iter()
            .map(|note| {
                let typed: TypedData = serde_json::from_value(note["typedData"].clone()).unwrap();
                let signature = signer.sign_hash_sync(&typed.eip712_signing_hash().unwrap()).unwrap();
                format!("0x{}", hex::encode(signature.as_bytes()))
            })
            .collect()
    }

    pub fn submit_all(id: &str, quote: &Value, signatures: &[String]) -> Request<Body> {
        post(format!("/v1/orders/{id}/legs"), json!({ "quoteId": quote["quoteId"], "signatures": signatures }))
    }

    #[sqlx::test]
    async fn a_signed_split_becomes_a_leg_and_a_lock_job_per_chain(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = split_quoted(&test, &alice).await;

        let (status, reply) = call(&test.app, submit_all(&id, &quote, &sign_all(&alice, &quote))).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{reply}");
        assert_eq!(reply["status"], "paying");
        assert_eq!(reply["locks"].as_array().unwrap().len(), 2);
        for (lock, note) in reply["locks"].as_array().unwrap().iter().zip(quote["toSign"].as_array().unwrap()) {
            assert_eq!((&lock["chainId"], &lock["lockId"]), (&note["chainId"], &note["lockId"]));
            assert!(lock["jobId"].is_i64());
        }

        let legs: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT chain_id, amount::text, lock_id FROM legs WHERE order_id = $1 ORDER BY chain_id")
                .bind(&id)
                .fetch_all(&test.pool)
                .await
                .unwrap();
        assert_eq!((legs[0].0, legs[0].1.as_str()), (84532, "6000000"));
        assert_eq!((legs[1].0, legs[1].1.as_str()), (11155111, "6036000"));
        let jobs: Vec<(i64, Value)> =
            sqlx::query_as("SELECT chain_id, args FROM relayer_jobs WHERE kind = 'source_lock' ORDER BY chain_id")
                .fetch_all(&test.pool)
                .await
                .unwrap();
        assert_eq!(jobs.len(), 2);
        for ((chain_id, args), (leg_chain, _, lock_id)) in jobs.iter().zip(&legs) {
            assert_eq!(chain_id, leg_chain);
            assert_eq!(args["lockId"].as_str(), Some(lock_id.as_str()));
        }

        let read = Request::get(format!("/v1/orders/{id}")).body(Body::empty()).unwrap();
        let (_, order) = call(&test.app, read).await;
        assert_eq!(order["path"], "split");
        assert_eq!(order["legs"].as_array().unwrap().len(), 2);
    }

    #[sqlx::test]
    async fn a_split_needs_one_good_signature_per_note(pool: PgPool) {
        let test = test_app(pool).await;
        let (alice, mallory) = (PrivateKeySigner::random(), PrivateKeySigner::random());
        let (id, quote) = split_quoted(&test, &alice).await;
        let mut signatures = sign_all(&alice, &quote);

        let (status, reply) = call(&test.app, submit_all(&id, &quote, &signatures[..1])).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_signature")));
        let (status, _) = call(&test.app, submit(&id, &quote, &signatures[0])).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        signatures.swap(0, 1);
        let (status, _) = call(&test.app, submit_all(&id, &quote, &signatures)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(&test.app, submit_all(&id, &quote, &sign_all(&mallory, &quote))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM relayer_jobs").fetch_one(&test.pool).await.unwrap();
        assert_eq!(jobs, 0);
    }

    #[sqlx::test]
    async fn signing_a_split_again_resends_only_the_lock_whose_job_failed(pool: PgPool) {
        let test = test_app(pool).await;
        let alice = PrivateKeySigner::random();
        let (id, quote) = split_quoted(&test, &alice).await;
        let signatures = sign_all(&alice, &quote);
        assert_eq!(call(&test.app, submit_all(&id, &quote, &signatures)).await.0, StatusCode::ACCEPTED);

        // A state the Relayer avoids but a race could leave: the order is back to `quoted` while one
        // lock job went through. Signing again must not send that lock twice.
        sqlx::query("UPDATE relayer_jobs SET status = 'failed' WHERE chain_id = 11155111").execute(&test.pool).await.unwrap();
        sqlx::query("UPDATE relayer_jobs SET status = 'confirmed', tx_hash = args->>'lockId' WHERE chain_id = 84532")
            .execute(&test.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE orders SET status = 'quoted'").execute(&test.pool).await.unwrap();

        let (status, reply) = call(&test.app, submit_all(&id, &quote, &signatures)).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{reply}");
        assert!(reply["locks"][0]["jobId"].is_null());
        assert_eq!(reply["locks"][1]["jobId"], reply["jobId"]);
        let jobs: Vec<(i64, String)> = sqlx::query_as("SELECT chain_id, status FROM relayer_jobs ORDER BY id")
            .fetch_all(&test.pool)
            .await
            .unwrap();
        assert_eq!(jobs.len(), 3);
        assert_eq!(jobs[2], (11155111, "pending".into()));

        // With every lock running or done there is nothing to send.
        sqlx::query("UPDATE orders SET status = 'quoted'").execute(&test.pool).await.unwrap();
        let (status, reply) = call(&test.app, submit_all(&id, &quote, &signatures)).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("already_submitted")));
        let order_status: String =
            sqlx::query_scalar("SELECT status FROM orders").fetch_one(&test.pool).await.unwrap();
        assert_eq!(order_status, "quoted");
    }
}
