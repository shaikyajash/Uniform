//! Matchmaker: watches the chains and acts for Bob, the liquidity provider. Owned by A.
//!
//! When a customer locks on a source chain for Bob, it opens the destination lock for the shop on
//! Whitechain with Bob's USDT. When the shop is paid and the secret becomes public, it redeems the
//! customer's source lock for Bob. It refunds locks that time out and reports every chain event to
//! the Order Desk. See docs/plans/2026-11-06-a5-matchmaker.md.

mod config;
mod events;
mod order_desk;
mod price;
mod rules;
mod store;
mod worker;

use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() {
    let config = match config::Config::from_env() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("matchmaker: {e}");
            std::process::exit(1);
        }
    };
    let poll = Duration::from_secs(config.poll_seconds);
    let worker = match worker::Worker::new(config).await {
        Ok(worker) => Arc::new(worker),
        Err(e) => {
            eprintln!("matchmaker: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("matchmaker: running as filler {}", worker.filler());

    // Source redeems run on their own loop, so slow scans or refunds can never delay them.
    let redeemer = Arc::clone(&worker);
    tokio::spawn(async move {
        loop {
            redeemer.redeem_duty().await;
            tokio::time::sleep(poll).await;
        }
    });

    loop {
        worker.tick().await;
        tokio::time::sleep(poll).await;
    }
}
