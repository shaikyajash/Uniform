//! `GET /health`: answers 200 while the Order Desk can reach its database, else 503.
//!
//! An outside cron calls it every few minutes, which keeps a free Render instance awake (it sleeps
//! after 15 minutes without traffic) and with it the Relayer, Matchmaker and Notifier that share
//! the container.

use axum::{extract::State, http::StatusCode};
use sqlx::PgPool;

pub async fn health(State(pool): State<PgPool>) -> (StatusCode, &'static str) {
    match sqlx::query("SELECT 1").execute(&pool).await {
        Ok(_) => (StatusCode::OK, "ok"),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "database unreachable"),
    }
}
