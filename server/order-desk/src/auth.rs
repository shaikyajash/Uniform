//! API keys: how a shop's server proves which shop it is.
//!
//! The key is shown to the shop once. Only its sha256 is stored (`merchants.api_key_hash`), so a
//! leaked database does not leak working keys.

use alloy::hex;
use axum::{
    extract::{FromRef, FromRequestParts},
    http::{header::AUTHORIZATION, request::Parts},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::error::ApiError;

const KEY_PREFIX: &str = "sk_test_";

/// A new random API key and the hash to store for it.
pub fn new_api_key() -> (String, String) {
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).expect("the operating system has no random source");
    let key = format!("{KEY_PREFIX}{}", hex::encode(secret));
    let hash = hash_api_key(&key);
    (key, hash)
}

pub fn hash_api_key(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

/// The shop behind a request's `Authorization: Bearer sk_test_...` header.
/// Add it as a handler argument; a missing or unknown key answers 401 before the handler runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shop {
    pub merchant_id: i64,
}

impl<S> FromRequestParts<S> for Shop
where
    PgPool: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let key = bearer_token(parts).ok_or_else(ApiError::unauthorized)?;
        let pool = PgPool::from_ref(state);
        let merchant_id: Option<i64> = sqlx::query_scalar("SELECT id FROM merchants WHERE api_key_hash = $1")
            .bind(hash_api_key(key))
            .fetch_optional(&pool)
            .await?;
        merchant_id.map(|merchant_id| Shop { merchant_id }).ok_or_else(ApiError::unauthorized)
    }
}

fn bearer_token(parts: &Parts) -> Option<&str> {
    let value = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
    value.strip_prefix("Bearer ").filter(|key| key.starts_with(KEY_PREFIX))
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
