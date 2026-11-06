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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Challenge {
    nonce: String,
    message: String,
    expires_at: i64,
}

pub async fn challenge(
    State(state): State<AppState>,
    body: Result<Json<ChallengeRequest>, JsonRejection>,
) -> Result<Json<Challenge>, ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let address: Address =
        body.address.trim().parse().map_err(|_| ApiError::bad_request("bad_address", "address must be a 0x address"))?;
    let nonce = hex::encode(&crate::random::bytes32()[..16]);
    let expires_at = now() + CHALLENGE_SECONDS;
    let message = challenge_message(&state.pay_base_url, address, &nonce);
    sqlx::query("INSERT INTO login_challenges (nonce, address, message, expires_at) VALUES ($1, $2, $3, $4)")
        .bind(&nonce)
        .bind(format!("{address:#x}"))
        .bind(&message)
        .bind(expires_at)
        .execute(&state.pool)
        .await?;
    Ok(Json(Challenge { nonce, message, expires_at }))
}

/// What the wallet shows. Plain words first: the owner should know it is free and moves nothing.
fn challenge_message(site: &str, address: Address, nonce: &str) -> String {
    let site = site.trim_end_matches('/');
    format!(
        "Sign in to the Uniform shop dashboard at {site}\n\n\
         Wallet: {}\n\n\
         This only proves you hold the shop's payout wallet. It is free and moves no money.\n\n\
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
    shop: ShopName,
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
    let Some((merchant_id, name, payout_address)) = shop else {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "not_a_shop", "no shop is paid to this wallet"));
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
    Ok(Json(SignedIn { token, expires_at, shop: ShopName { name, payout_address } }))
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
        assert_eq!(reply["shop"]["name"], "Kettle & Co");
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
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("not_a_shop")));
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
