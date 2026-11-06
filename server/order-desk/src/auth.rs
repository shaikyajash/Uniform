//! Who is calling: a shop's server (API key) or a shop's owner in the dashboard (session).
//!
//! - **API key** `sk_test_...`: shown to the shop once. Only its sha256 is stored
//!   (`merchants.api_key_hash`), so a leaked database does not leak working keys.
//! - **Session** `sess_...`: given after the owner signs in with the payout wallet
//!   (`routes/login.rs`). Also stored only as a sha256, and it runs out.
//!
//! [`Shop`] accepts either. [`Owner`] accepts only a session: changing settings and refunding are
//! for the person holding the payout wallet, not for whoever has the server's key.

use alloy::hex;
use axum::{
    extract::{FromRef, FromRequestParts},
    http::{header::AUTHORIZATION, request::Parts},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::error::ApiError;

const KEY_PREFIX: &str = "sk_test_";
const SESSION_PREFIX: &str = "sess_";

/// A new random API key and the hash to store for it.
pub fn new_api_key() -> (String, String) {
    let key = format!("{KEY_PREFIX}{}", hex::encode(crate::random::bytes32()));
    let hash = hash_api_key(&key);
    (key, hash)
}

pub fn hash_api_key(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

/// A new random session token and the hash to store for it.
pub fn new_session_token() -> (String, String) {
    let token = format!("{SESSION_PREFIX}{}", hex::encode(crate::random::bytes32()));
    let hash = hash_api_key(&token);
    (token, hash)
}

/// The shop behind a request's `Authorization: Bearer ...` header: its API key or a dashboard
/// session. Add it as a handler argument; a missing or unknown credential answers 401 before the
/// handler runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shop {
    pub merchant_id: i64,
}

/// The shop's owner, signed in to the dashboard with the payout wallet. Never an API key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub merchant_id: i64,
    /// sha256 of the session token, to sign out.
    pub session_hash: String,
}

impl<S> FromRequestParts<S> for Shop
where
    PgPool: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let token = bearer_token(parts).ok_or_else(ApiError::unauthorized)?;
        let pool = PgPool::from_ref(state);
        if token.starts_with(SESSION_PREFIX) {
            let owner = session_owner(&pool, token).await?;
            return Ok(Shop { merchant_id: owner.merchant_id });
        }
        if !token.starts_with(KEY_PREFIX) {
            return Err(ApiError::unauthorized());
        }
        let merchant_id: Option<i64> = sqlx::query_scalar("SELECT id FROM merchants WHERE api_key_hash = $1")
            .bind(hash_api_key(token))
            .fetch_optional(&pool)
            .await?;
        merchant_id.map(|merchant_id| Shop { merchant_id }).ok_or_else(ApiError::unauthorized)
    }
}

impl<S> FromRequestParts<S> for Owner
where
    PgPool: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let token = bearer_token(parts).filter(|token| token.starts_with(SESSION_PREFIX));
        session_owner(&PgPool::from_ref(state), token.ok_or_else(ApiError::unauthorized)?).await
    }
}

async fn session_owner(pool: &PgPool, token: &str) -> Result<Owner, ApiError> {
    let session_hash = hash_api_key(token);
    let merchant_id: Option<i64> =
        sqlx::query_scalar("SELECT merchant_id FROM sessions WHERE token_hash = $1 AND expires_at > $2")
            .bind(&session_hash)
            .bind(crate::routes::now())
            .fetch_optional(pool)
            .await?;
    merchant_id.map(|merchant_id| Owner { merchant_id, session_hash }).ok_or_else(ApiError::unauthorized)
}

fn bearer_token(parts: &Parts) -> Option<&str> {
    let value = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
    value.strip_prefix("Bearer ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Request, StatusCode};

    async fn add_shop(pool: &PgPool, key_hash: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO merchants (name, payout_address, api_key_hash, webhook_secret, created_at)
             VALUES ('Kettle & Co', '0x5a30000000000000000000000000000000000001', $1, 'whsec_test', 1)
             RETURNING id",
        )
        .bind(key_hash)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn shop_for(pool: &PgPool, header: Option<&str>) -> Result<Shop, StatusCode> {
        let mut request = Request::builder().uri("/v1/orders");
        if let Some(value) = header {
            request = request.header(AUTHORIZATION, value);
        }
        let (mut parts, ()) = request.body(()).unwrap().into_parts();
        Shop::from_request_parts(&mut parts, pool).await.map_err(|error| error.status)
    }

    #[test]
    fn keys_are_random_prefixed_and_stored_only_as_hashes() {
        let (key, hash) = new_api_key();
        let (other_key, _) = new_api_key();
        assert!(key.starts_with("sk_test_") && key.len() == 8 + 64, "{key}");
        assert_ne!(key, other_key);
        assert_eq!(hash, hash_api_key(&key));
        assert!(!hash.contains(&key[8..]));
    }

    #[sqlx::test]
    async fn the_right_key_finds_its_shop(pool: PgPool) {
        let (key, hash) = new_api_key();
        let id = add_shop(&pool, &hash).await;
        assert_eq!(shop_for(&pool, Some(&format!("Bearer {key}"))).await, Ok(Shop { merchant_id: id }));
    }

    async fn owner_for(pool: &PgPool, header: &str) -> Result<i64, StatusCode> {
        let request = Request::builder().uri("/v1/shop").header(AUTHORIZATION, header);
        let (mut parts, ()) = request.body(()).unwrap().into_parts();
        Owner::from_request_parts(&mut parts, pool).await.map(|owner| owner.merchant_id).map_err(|error| error.status)
    }

    #[sqlx::test]
    async fn a_live_session_is_the_owner_and_an_api_key_is_not(pool: PgPool) {
        let (key, hash) = new_api_key();
        let id = add_shop(&pool, &hash).await;
        let (token, token_hash) = new_session_token();
        let (old, old_hash) = new_session_token();
        for (hash, expires_at) in [(&token_hash, i64::MAX), (&old_hash, 1)] {
            sqlx::query("INSERT INTO sessions (token_hash, merchant_id, created_at, expires_at) VALUES ($1, $2, 1, $3)")
                .bind(hash)
                .bind(id)
                .bind(expires_at)
                .execute(&pool)
                .await
                .unwrap();
        }
        assert_eq!(owner_for(&pool, &format!("Bearer {token}")).await, Ok(id));
        assert_eq!(shop_for(&pool, Some(&format!("Bearer {token}"))).await, Ok(Shop { merchant_id: id }));
        assert_eq!(owner_for(&pool, &format!("Bearer {key}")).await, Err(StatusCode::UNAUTHORIZED), "an API key");
        assert_eq!(owner_for(&pool, &format!("Bearer {old}")).await, Err(StatusCode::UNAUTHORIZED), "expired");
    }

    #[sqlx::test]
    async fn missing_wrong_or_malformed_keys_get_401(pool: PgPool) {
        let (key, hash) = new_api_key();
        add_shop(&pool, &hash).await;
        let (stranger, _) = new_api_key();
        for header in [None, Some(format!("Bearer {stranger}")), Some(format!("Basic {key}")), Some("Bearer abc".into())] {
            assert_eq!(shop_for(&pool, header.as_deref()).await, Err(StatusCode::UNAUTHORIZED), "{header:?}");
        }
    }
}
