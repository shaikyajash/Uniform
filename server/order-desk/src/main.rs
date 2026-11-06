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
mod refunds;
mod routes;

use std::{env, sync::Arc};

use common::{amount::Amount, chains::Chains};
use sqlx::PgPool;

const DEFAULT_PORT: u16 = 4000;
const DEFAULT_PAY_BASE_URL: &str = "http://localhost:3000";
/// Relative to `server/`, where `cargo run` starts.
const DEFAULT_CHAINS_JSON: &str = "../shared/chains.json";
const MIN_INTERNAL_TOKEN_LENGTH: usize = 32;
/// The same setting and default as the Relayer's: it will not pay gas for a larger order.
const DEFAULT_MAX_ORDER_USDT: &str = "100";
const USDT_DECIMALS: u8 = 6;

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
    let internal_token = env::var("INTERNAL_EVENTS_TOKEN").unwrap_or_default();
    if internal_token.len() < MIN_INTERNAL_TOKEN_LENGTH {
        return Err(format!(
            "set INTERNAL_EVENTS_TOKEN to a random secret of at least {MIN_INTERNAL_TOKEN_LENGTH} characters \
             (the Matchmaker sends it), e.g. openssl rand -hex 32"
        ));
    }
    let max_order_text = env::var("RELAYER_MAX_ORDER_USDT").unwrap_or_else(|_| DEFAULT_MAX_ORDER_USDT.to_owned());
    let max_order = Amount::from_decimal(&max_order_text, USDT_DECIMALS)
        .map_err(|_| format!("RELAYER_MAX_ORDER_USDT must be a number of USDT, got {max_order_text:?}"))?;
    let provider = match env::var("PROVIDER_ADDRESS") {
        Ok(text) if !text.trim().is_empty() => Some(
            text.trim()
                .parse()
                .map_err(|_| format!("PROVIDER_ADDRESS must be Bob's 0x address, got {text:?}"))?,
        ),
        _ => {
            println!("order-desk: PROVIDER_ADDRESS is not set; only Path A (USDT on Whitechain) is offered");
            None
        }
    };
    tokio::spawn(refunds::run(pool.clone()));
    let app = routes::router(routes::AppState {
        pool,
        chains: Arc::new(chains),
        pay_base_url,
        internal_token,
        max_order,
        provider,
    });
    // 0.0.0.0, not 127.0.0.1: a host like Railway reaches the server from outside the container.
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|error| format!("cannot listen on port {port}: {error}"))?;
    println!("order-desk: listening on port {port}");
    axum::serve(listener, app).await.map_err(|error| error.to_string())
}
