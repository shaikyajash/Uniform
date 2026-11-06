//! Signing in to the dashboard with the shop's payout wallet.
//!
//! 1. `POST /v1/auth/challenge {address}`: a one-time message to sign, valid for 5 minutes.
//! 2. The owner signs it in their wallet (`personal_sign`). Signing costs nothing and moves nothing.
//! 3. `POST /v1/auth/verify {nonce, signature}`: if the signature recovers to a shop's payout
//!    address, a session token valid for 12 hours.
//! 4. `POST /v1/auth/logout`: ends the session.
//!
//! The message names this site and a random nonce, so a signature cannot be reused elsewhere or
//! twice. Only ordinary wallets (65-byte signatures) can sign in; smart-contract wallets cannot.
//!
//! Signing up is the same two calls: a challenge with `createName` asks the wallet to sign "create
//! a merchant named …", and `verify` then makes that merchant in the same transaction as the
//! session and returns its API key and webhook secret once (`created`). The name is read back from
//! the stored, signed message, so it is exactly what the wallet agreed to.

use alloy::{
    hex,
    primitives::{Address, Bytes, Signature},
};
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};

use super::{AppState, now};
use crate::{
    auth::{Owner, new_session_token},
    error::ApiError,
};

const CHALLENGE_SECONDS: i64 = 5 * 60;
const SESSION_SECONDS: i64 = 12 * 3600;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChallengeRequest {
    address: String,
    /// Sign up: the new merchant's name. The message then asks to create it.
    #[serde(default)]
    create_name: Option<String>,
}

/// The longest merchant name sign-up accepts.
const MAX_NAME: usize = 60;
const CREATE_PREFIX: &str = "Create a Uniform merchant account at ";
const NAME_LINE: &str = "Merchant: ";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Challenge {
    nonce: String,
    message: String,
    expires_at: i64,
    /// Whether this wallet is already a merchant's payout wallet, so the page knows to ask for a
    /// name (sign up) before the one signature.
    merchant: bool,
}

pub async fn challenge(
    State(state): State<AppState>,
    body: Result<Json<ChallengeRequest>, JsonRejection>,
) -> Result<Json<Challenge>, ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let address: Address =
        body.address.trim().parse().map_err(|_| ApiError::bad_request("bad_address", "address must be a 0x address"))?;
    let name = match body.create_name.as_deref().map(str::trim) {
        None => None,
        Some(name) if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) => {
            return Err(ApiError::bad_request("bad_name", format!("the name must be 1 to {MAX_NAME} characters on one line")));
        }
        Some(name) => Some(name.to_owned()),
    };
    let merchant: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM merchants WHERE payout_address = $1)")
        .bind(format!("{address:#x}"))
        .fetch_one(&state.pool)
        .await?;
    let nonce = hex::encode(&crate::random::bytes32()[..16]);
    let expires_at = now() + CHALLENGE_SECONDS;
    let message = match &name {
        Some(name) => sign_up_message(&state.pay_base_url, address, name, &nonce),
        None => challenge_message(&state.pay_base_url, address, &nonce),
    };
    sqlx::query("INSERT INTO login_challenges (nonce, address, message, expires_at) VALUES ($1, $2, $3, $4)")
        .bind(&nonce)
        .bind(format!("{address:#x}"))
        .bind(&message)
        .bind(expires_at)
        .execute(&state.pool)
        .await?;
    Ok(Json(Challenge { nonce, message, expires_at, merchant }))
}

/// What the wallet shows when signing up: the name it creates, then the same plain words.
fn sign_up_message(site: &str, address: Address, name: &str, nonce: &str) -> String {
    let site = site.trim_end_matches('/');
    format!(
        "{CREATE_PREFIX}{site}\n\n\
         {NAME_LINE}{name}\n\
         Payout wallet: {}\n\n\
         Payments to this merchant are paid in USDT to this wallet. Signing is free and moves no money.\n\n\
         Nonce: {nonce}",
        address.to_checksum(None)
    )
}

/// The merchant name a sign-up message asked for, or `None` for a plain sign-in message.
fn signed_up_name(message: &str) -> Option<&str> {
    if !message.starts_with(CREATE_PREFIX) {
        return None;
    }
    message.lines().find_map(|line| line.strip_prefix(NAME_LINE))
}

/// What the wallet shows. Plain words first: the owner should know it is free and moves nothing.
fn challenge_message(site: &str, address: Address, nonce: &str) -> String {
    let site = site.trim_end_matches('/');
    format!(
        "Sign in to the Uniform merchant dashboard at {site}\n\n\
         Wallet: {}\n\n\
         This only proves you hold the merchant's payout wallet. It is free and moves no money.\n\n\
         Nonce: {nonce}",
        address.to_checksum(None)
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifyRequest {
    nonce: String,
    /// 65 bytes as 0x hex, from `personal_sign` of the challenge message.
    signature: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedIn {
    token: String,
    expires_at: i64,
    merchant: ShopName,
    /// Only right after sign-up: the new merchant's secrets, shown this once.
    #[serde(skip_serializing_if = "Option::is_none")]
    created: Option<Created>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    api_key: String,
    webhook_secret: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShopName {
    name: String,
    payout_address: String,
}

pub async fn verify(
    State(state): State<AppState>,
    body: Result<Json<VerifyRequest>, JsonRejection>,
) -> Result<Json<SignedIn>, ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let bad = |message: &str| ApiError::bad_request("bad_signature", message.to_owned());
    let mut tx = state.pool.begin().await?;
    // Used once: marked here, inside the same transaction that makes the session.
    let challenge: Option<(String, String)> = sqlx::query_as(
        "UPDATE login_challenges SET used = TRUE
         WHERE nonce = $1 AND NOT used AND expires_at > $2
         RETURNING address, message",
    )
    .bind(&body.nonce)
    .bind(now())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((address, message)) = challenge else {
        return Err(ApiError::conflict("challenge_expired", "that sign-in message ran out or was used; start again"));
    };

    let bytes: Bytes = body.signature.parse().map_err(|_| bad("signature must be 0x hex"))?;
    if bytes.len() != 65 {
        return Err(bad("signature must be 65 bytes (smart-contract wallets cannot sign in)"));
    }
    let signature = Signature::from_raw(&bytes).map_err(|_| bad("signature is not a valid r, s, v"))?;
    let signer = signature.recover_address_from_msg(message.as_bytes()).map_err(|_| bad("signature does not recover"))?;
    if format!("{signer:#x}") != address {
        return Err(bad("the signature is from another wallet"));
    }

    let shop: Option<(i64, String, String)> =
        sqlx::query_as("SELECT id, name, payout_address FROM merchants WHERE payout_address = $1")
            .bind(&address)
            .fetch_optional(&mut *tx)
            .await?;
    let (merchant_id, name, payout_address, created) = match (shop, signed_up_name(&message)) {
        (Some((id, name, payout)), _) => (id, name, payout, None),
        (None, Some(new_name)) => {
            let signer_address: Address = address.parse().map_err(ApiError::internal)?;
            let new = crate::admin::add_shop(&mut *tx, new_name, signer_address).await?;
            let created = Created { api_key: new.api_key, webhook_secret: new.webhook_secret };
            (new.id, new_name.to_owned(), address.clone(), Some(created))
        }
        (None, None) => {
            return Err(ApiError::new(StatusCode::FORBIDDEN, "not_a_merchant", "no merchant is paid to this wallet"));
        }
    };
    let (token, token_hash) = new_session_token();
    let created_at = now();
    let expires_at = created_at + SESSION_SECONDS;
    sqlx::query("INSERT INTO sessions (token_hash, merchant_id, created_at, expires_at) VALUES ($1, $2, $3, $4)")
        .bind(token_hash)
        .bind(merchant_id)
        .bind(created_at)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(SignedIn { token, expires_at, merchant: ShopName { name, payout_address }, created }))
}

pub async fn logout(State(state): State<AppState>, owner: Owner) -> Result<StatusCode, ApiError> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = $1").bind(owner.session_hash).execute(&state.pool).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use alloy::signers::{SignerSync, local::PrivateKeySigner};
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use serde_json::{Value, json};
    use sqlx::PgPool;

    use crate::routes::test_support::{TestApp, call, test_app};

    async fn post(app: &TestApp, path: &str, body: Value, token: Option<&str>) -> (StatusCode, Value) {
        let mut request = Request::post(path).header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        call(&app.app, request.body(Body::from(body.to_string())).unwrap()).await
    }

    async fn sign_in(app: &TestApp, wallet: &PrivateKeySigner) -> (StatusCode, Value, String) {
        let (_, challenge) = post(app, "/v1/auth/challenge", json!({ "address": wallet.address() }), None).await;
        let message = challenge["message"].as_str().unwrap();
        let signature = wallet.sign_message_sync(message.as_bytes()).unwrap();
        let nonce = challenge["nonce"].as_str().unwrap().to_owned();
        let body = json!({ "nonce": nonce, "signature": signature.to_string() });
        let (status, reply) = post(app, "/v1/auth/verify", body, None).await;
        (status, reply, nonce)
    }

    async fn shop_wallet(app: &TestApp) -> PrivateKeySigner {
        let wallet = PrivateKeySigner::random();
        sqlx::query("UPDATE merchants SET payout_address = $1").bind(format!("{:#x}", wallet.address())).execute(&app.pool).await.unwrap();
        wallet
    }

    #[sqlx::test]
    async fn the_payout_wallet_signs_in_once_and_out(pool: PgPool) {
        let app = test_app(pool).await;
        let wallet = shop_wallet(&app).await;
        let (status, reply, nonce) = sign_in(&app, &wallet).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!(reply["merchant"]["name"], "Kettle & Co");
        let token = reply["token"].as_str().unwrap().to_owned();
        assert!(token.starts_with("sess_"));

        let again = json!({ "nonce": nonce, "signature": format!("0x{}", "11".repeat(65)) });
        assert_eq!(post(&app, "/v1/auth/verify", again, None).await.0, StatusCode::CONFLICT, "a nonce works once");

        assert_eq!(post(&app, "/v1/auth/logout", json!({}), Some(&token)).await.0, StatusCode::NO_CONTENT);
        assert_eq!(post(&app, "/v1/auth/logout", json!({}), Some(&token)).await.0, StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test]
    async fn another_wallet_is_not_a_shop(pool: PgPool) {
        let app = test_app(pool).await;
        let (status, reply, _) = sign_in(&app, &PrivateKeySigner::random()).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("not_a_merchant")));
    }

    #[sqlx::test]
    async fn a_new_wallet_signs_up_once_and_then_signs_in(pool: PgPool) {
        let app = test_app(pool).await;
        let wallet = PrivateKeySigner::random();
        let ask = json!({ "address": wallet.address(), "createName": "  Chai Point " });
        let (status, challenge) = post(&app, "/v1/auth/challenge", ask, None).await;
        assert_eq!(status, StatusCode::OK, "{challenge}");
        assert_eq!(challenge["merchant"], false);
        let message = challenge["message"].as_str().unwrap();
        assert!(message.contains("Merchant: Chai Point\n"), "{message}");
        let signature = wallet.sign_message_sync(message.as_bytes()).unwrap();
        let body = json!({ "nonce": challenge["nonce"], "signature": signature.to_string() });
        let (status, reply) = post(&app, "/v1/auth/verify", body, None).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!(reply["merchant"]["name"], "Chai Point");
        assert!(reply["created"]["apiKey"].as_str().is_some_and(|key| !key.is_empty()));
        assert!(reply["created"]["webhookSecret"].as_str().is_some_and(|secret| secret.starts_with("whsec_")));

        let (status, reply, _) = sign_in(&app, &wallet).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert!(reply.get("created").is_none(), "secrets are shown only at sign-up");
        let (_, again) = post(&app, "/v1/auth/challenge", json!({ "address": wallet.address() }), None).await;
        assert_eq!(again["merchant"], true);
    }

    #[sqlx::test]
    async fn a_bad_merchant_name_is_refused(pool: PgPool) {
        let app = test_app(pool).await;
        for name in ["", "   ", "two\nlines", &"x".repeat(61)] {
            let ask = json!({ "address": PrivateKeySigner::random().address(), "createName": name });
            let (status, reply) = post(&app, "/v1/auth/challenge", ask, None).await;
            assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_name")), "{name:?}");
        }
    }

    #[sqlx::test]
    async fn a_signature_from_another_wallet_fails(pool: PgPool) {
        let app = test_app(pool).await;
        let wallet = shop_wallet(&app).await;
        let (_, challenge) = post(&app, "/v1/auth/challenge", json!({ "address": wallet.address() }), None).await;
        let forged = PrivateKeySigner::random().sign_message_sync(challenge["message"].as_str().unwrap().as_bytes()).unwrap();
        let body = json!({ "nonce": challenge["nonce"], "signature": forged.to_string() });
        let (status, reply) = post(&app, "/v1/auth/verify", body, None).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_signature")));
    }
}
