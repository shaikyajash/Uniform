//! Shop Notifier: tells shops when an order is paid, refunded or expired, with a signed webhook.
//!
//! It reads the Order Desk's database and never takes requests. Every `NOTIFIER_POLL_SECONDS`:
//! 1. **Queue:** each order that reached a final status gets one row in `webhook_log` per event,
//!    with the exact JSON to send (SPEC "Webhook"). The status alone decides, so it does not matter
//!    who moved the order (an event, the Relayer, the expiry sweep).
//! 2. **Deliver:** rows that are due are POSTed to the shop's `webhook_url` with
//!    `X-Uniform-Signature: hex(HMAC_SHA256(body, webhook secret))`. A 2xx is delivered; anything
//!    else is tried again after 10 s, 1 min, 5 min and 30 min, then marked failed.
//!
//! Run one Notifier: two would each send a due row once.
//!
//! Environment: `DATABASE_URL`, `CHAINS_JSON` (default `../shared/chains.json`),
//! `NOTIFIER_POLL_SECONDS` (default 2).

mod deliver;
mod queue;

use std::{
    env,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use common::chains::Chains;
use sqlx::postgres::PgPoolOptions;

const DEFAULT_CHAINS_JSON: &str = "../shared/chains.json";

#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("notifier: {message}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let database_url = env::var("DATABASE_URL").map_err(|_| "set DATABASE_URL to the Order Desk's Postgres")?;
    let chains_path = env::var("CHAINS_JSON").unwrap_or_else(|_| DEFAULT_CHAINS_JSON.to_owned());
    let chains = Chains::load(&chains_path).map_err(|error| error.to_string())?;
    let poll = match env::var("NOTIFIER_POLL_SECONDS") {
        Ok(text) => text.parse().map_err(|_| format!("NOTIFIER_POLL_SECONDS must be a number, got {text:?}"))?,
        Err(_) => 2,
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .map_err(|error| format!("cannot connect to Postgres: {error}"))?;
    let http = reqwest::Client::builder()
        .timeout(deliver::TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| error.to_string())?;

    println!("notifier: watching orders every {poll} s");
    loop {
        let now = unix_now();
        match queue::queue(&pool, &chains, now).await {
            Ok(0) => {}
            Ok(count) => println!("notifier: queued {count} webhook(s)"),
            Err(error) => eprintln!("notifier: queueing failed: {error}"),
        }
        match deliver::deliver_due(&pool, &http, now).await {
            Ok(lines) => lines.iter().for_each(|line| println!("notifier: {line}")),
            Err(error) => eprintln!("notifier: delivery failed: {error}"),
        }
        tokio::time::sleep(Duration::from_secs(poll)).await;
    }
}

fn unix_now() -> i64 {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
    i64::try_from(elapsed).unwrap_or(i64::MAX)
}
