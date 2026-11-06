//! The HTTP API: shared state and the route table.

pub mod orders;

use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    extract::FromRef,
    routing::{get, post},
};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// Where the pay page lives; `payUrl` is this plus `/pay/{orderId}`.
    pub pay_base_url: String,
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
        .with_state(state)
}

/// Unix seconds, the time unit of the database and the API.
pub fn now() -> i64 {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is before 1970");
    i64::try_from(elapsed.as_secs()).expect("the clock is past the year 292 billion")
}
