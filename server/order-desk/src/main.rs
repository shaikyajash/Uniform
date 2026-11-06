//! Order Desk: the API and database.
//!
//! `order-desk` (or `order-desk serve`) runs the API.
//! `order-desk add-shop --name ... --payout 0x...` registers a shop and prints its API key.

mod admin;
mod auth;
mod db;
mod direct_pay;
mod error;
mod random;
mod routes;

use std::{env, sync::Arc};

use common::chains::Chains;
use sqlx::PgPool;

const DEFAULT_PORT: u16 = 4000;
const DEFAULT_PAY_BASE_URL: &str = "http://localhost:3000";
/// Relative to `server/`, where `cargo run` starts.
const DEFAULT_CHAINS_JSON: &str = "../shared/chains.json";

#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("order-desk: {message}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let database_url =
        env::var("DATABASE_URL").map_err(|_| "set DATABASE_URL, e.g. postgres://you@localhost/uniform".to_owned())?;
    let pool = db::connect(&database_url).await.map_err(|error| error.to_string())?;
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") => serve(pool).await,
        Some("add-shop") => admin::add_shop_command(&pool, &args[1..]).await,
        Some(other) => Err(format!("unknown command {other:?}: use serve or add-shop")),
    }
}

async fn serve(pool: PgPool) -> Result<(), String> {
    let port = match env::var("PORT") {
        Ok(port) => port.parse().map_err(|_| format!("PORT must be a number, got {port:?}"))?,
        Err(_) => DEFAULT_PORT,
    };
    let pay_base_url = env::var("PAY_BASE_URL").unwrap_or_else(|_| DEFAULT_PAY_BASE_URL.to_owned());
    let chains_path = env::var("CHAINS_JSON").unwrap_or_else(|_| DEFAULT_CHAINS_JSON.to_owned());
    let chains = Chains::load(&chains_path).map_err(|error| error.to_string())?;
    let app = routes::router(routes::AppState { pool, chains: Arc::new(chains), pay_base_url });
    // 0.0.0.0, not 127.0.0.1: a host like Railway reaches the server from outside the container.
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|error| format!("cannot listen on port {port}: {error}"))?;
    println!("order-desk: listening on port {port}");
    axum::serve(listener, app).await.map_err(|error| error.to_string())
}
