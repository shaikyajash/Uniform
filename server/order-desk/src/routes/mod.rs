//! The HTTP API: shared state and the route table.

pub mod direct;
pub mod events;
pub mod orders;
pub mod quote;

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    extract::FromRef,
    routing::{get, post},
};
use common::chains::Chains;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// `shared/chains.json`: addresses, tokens and timeouts.
    pub chains: Arc<Chains>,
    /// Where the pay page lives; `payUrl` is this plus `/pay/{orderId}`.
    pub pay_base_url: String,
    /// Shared secret the Matchmaker sends on `/internal/events` (`INTERNAL_EVENTS_TOKEN`).
    pub internal_token: String,
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> PgPool {
        state.pool.clone()
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/orders", post(orders::create))
        .route("/v1/orders/{id}", get(orders::read))
        .route("/v1/orders/{id}/quote", post(quote::quote))
        .route("/v1/orders/{id}/direct", post(direct::direct))
        .route("/internal/events", post(events::report))
        .with_state(state)
}

/// Unix seconds, the time unit of the database and the API.
pub fn now() -> i64 {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is before 1970");
    i64::try_from(elapsed.as_secs()).expect("the clock is past the year 292 billion")
}

/// An order id from a URL in the database's form (lowercase `0x` + 64 hex), or `None`.
pub fn normalize_order_id(id: &str) -> Option<String> {
    let id = id.to_ascii_lowercase();
    let is_hex = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
    (id.len() == 66 && id.starts_with("0x") && id.bytes().skip(2).all(is_hex)).then_some(id)
}

#[cfg(test)]
pub mod test_support {
    use std::sync::Arc;

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use common::chains::Chains;
    use serde_json::Value;
    use sqlx::PgPool;
    use tower::ServiceExt;

    use super::{AppState, router};
    use crate::auth::new_api_key;

    const CHAINS_JSON: &str = include_str!("../../../../shared/chains.json");
    pub const INTERNAL_TOKEN: &str = "test-internal-token-0123456789";

    pub struct TestApp {
        pub app: Router,
        pub pool: PgPool,
        pub key: String,
    }

    impl TestApp {
        pub async fn create_order(&self, body: Value) -> (StatusCode, Value) {
            let request = Request::post("/v1/orders")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {}", self.key))
                .body(Body::from(body.to_string()))
                .unwrap();
            call(&self.app, request).await
        }
    }

    /// The real router and chains.json, with one shop "Kettle & Co" paid at 0x5a30...0001.
    pub async fn test_app(pool: PgPool) -> TestApp {
        let (key, hash) = new_api_key();
        sqlx::query(
            "INSERT INTO merchants (name, payout_address, api_key_hash, webhook_secret, created_at)
             VALUES ('Kettle & Co', '0x5a30000000000000000000000000000000000001', $1, 'whsec_test', 1)",
        )
        .bind(hash)
        .execute(&pool)
        .await
        .unwrap();
        let state = AppState {
            pool: pool.clone(),
            chains: Arc::new(Chains::from_json(CHAINS_JSON).unwrap()),
            pay_base_url: "https://pay.example/".into(),
            internal_token: INTERNAL_TOKEN.into(),
        };
        TestApp { app: router(state), pool, key }
    }

    pub async fn call(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}
