//! Relayer: sends the transactions the Order Desk asks for and pays their gas.
//!
//! It reads jobs from the Order Desk's `relayer_jobs` table and never takes requests from outside.
//! One worker per chain, each with its own nonces and gas budget, so a slow chain never holds up
//! another:
//! - the shop chain (Whitechain): `DirectPay.pay` (Path A) and `DestinationLock.claim`;
//! - each customer chain whose SourceLock is in chains.json: `SourceLock.lockWithAuthorization`
//!   and `SourceLock.refund`.
//!
//! The same key and address on every chain; it needs only each chain's gas coin.
//!
//! Environment:
//! - `DATABASE_URL`: the Order Desk's Postgres (the Order Desk runs the migrations).
//! - `RELAYER_PRIVATE_KEY`: the key that signs and pays gas. Never printed.
//! - `CHAINS_JSON`: default `../shared/chains.json`.
//! - `RPC_<chainId>`: optional; overrides a chain's RPC in chains.json (e.g. a local anvil copy).
//!   `SHOP_CHAIN_RPC` still works for the shop chain.
//! - `RELAYER_MAX_ORDER_USDT`: the largest order it pays gas for, default 100.
//! - `RELAYER_DAILY_GAS_WEI`: gas per shop per chain per UTC day, default 0.05 of the chain's coin.

mod chain;
mod worker;

use std::{
    env,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use alloy::{primitives::U256, signers::local::PrivateKeySigner};
use common::{amount::Amount, chains::{Chain as ChainConfig, Chains}};
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::{
    chain::{Chain, Rpc},
    worker::{Limits, Outcome},
};

const DEFAULT_CHAINS_JSON: &str = "../shared/chains.json";
const DEFAULT_MAX_ORDER_USDT: &str = "100";
/// 0.05 of the chain's coin. A `DirectPay.pay` costs well under 0.001 WBT on Whitechain testnet.
const DEFAULT_DAILY_GAS_WEI: &str = "50000000000000000";
/// How long to sleep when there is no job to do.
const IDLE: Duration = Duration::from_secs(1);
const MAX_CONNECTIONS: u32 = 6;

#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("relayer: {message}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let database_url = env::var("DATABASE_URL").map_err(|_| "set DATABASE_URL to the Order Desk's Postgres")?;
    let chains_path = env::var("CHAINS_JSON").unwrap_or_else(|_| DEFAULT_CHAINS_JSON.to_owned());
    let chains = Chains::load(&chains_path).map_err(|error| error.to_string())?;
    let shop_chain = chains.shop_chain();

    let signer: PrivateKeySigner = env::var("RELAYER_PRIVATE_KEY")
        .map_err(|_| "set RELAYER_PRIVATE_KEY (64 hex characters)")?
        .trim()
        .parse()
        // The parse error could echo the key, so it is not shown.
        .map_err(|_| "RELAYER_PRIVATE_KEY is not a valid private key")?;

    let usdt_decimals = shop_chain.tokens.get("USDT").ok_or("chains.json: no USDT on the shop chain")?.decimals;
    let max_order_text = env::var("RELAYER_MAX_ORDER_USDT").unwrap_or_else(|_| DEFAULT_MAX_ORDER_USDT.to_owned());
    let max_order = Amount::from_decimal(&max_order_text, usdt_decimals)
        .map_err(|_| format!("RELAYER_MAX_ORDER_USDT must be a number of USDT, got {max_order_text:?}"))?;
    let daily_gas_text = env::var("RELAYER_DAILY_GAS_WEI").unwrap_or_else(|_| DEFAULT_DAILY_GAS_WEI.to_owned());
    let daily_gas_wei: U256 = daily_gas_text
        .parse::<Amount>()
        .map_err(|_| format!("RELAYER_DAILY_GAS_WEI must be a whole number of wei, got {daily_gas_text:?}"))?
        .0;

    // The shop chain always; a customer chain once its SourceLock is deployed.
    let served: Vec<&ChainConfig> = std::iter::once(shop_chain)
        .chain(chains.customer_chains().filter(|chain| chain.contracts.source_lock.is_some()))
        .collect();
    let pool = PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .connect(&database_url)
        .await
        .map_err(|error| format!("cannot connect to Postgres: {error}"))?;

    let mut workers = tokio::task::JoinSet::new();
    for config in served {
        let rpc_url = rpc_url(config, config.chain_id == shop_chain.chain_id);
        let chain = Rpc::connect(config, &rpc_url, signer.clone())?;
        let limits = Limits { chain_id: config.chain_id, max_order, daily_gas_wei };
        println!(
            "relayer: sending from {} on chain {} via {}; max order {max_order_text} USDT",
            chain.sender(),
            limits.chain_id,
            host_of(&rpc_url)
        );
        workers.spawn(serve(pool.clone(), chain, limits));
    }
    // Workers run forever; one ending means something is badly wrong.
    match workers.join_next().await {
        Some(Ok(())) | None => Err("a chain worker stopped".to_owned()),
        Some(Err(error)) => Err(format!("a chain worker crashed: {error}")),
    }
}

/// `RPC_<chainId>`, then (for the shop chain) `SHOP_CHAIN_RPC`, then chains.json.
fn rpc_url(config: &ChainConfig, is_shop_chain: bool) -> String {
    let set = |name: String| env::var(name).ok().filter(|url| !url.trim().is_empty());
    set(format!("RPC_{}", config.chain_id))
        .or_else(|| if is_shop_chain { set("SHOP_CHAIN_RPC".to_owned()) } else { None })
        .unwrap_or_else(|| config.rpc.clone())
}

/// Only the host: the full RPC address may carry an API key or a password.
fn host_of(rpc_url: &str) -> &str {
    rpc_url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', '?']).next())
        .and_then(|authority| authority.rsplit('@').next())
        .unwrap_or("?")
}

/// One chain's jobs, one at a time, forever.
async fn serve(pool: PgPool, chain: Rpc, limits: Limits) {
    loop {
        match worker::step(&pool, &chain, &limits, unix_now()).await {
            Ok(Some(report)) => {
                let line = match report.outcome {
                    Outcome::Sent(hash) => format!("sent {hash}"),
                    Outcome::Confirmed(hash) => format!("confirmed {hash}"),
                    Outcome::Failed(reason) => format!("failed: {reason}"),
                    Outcome::Waiting(reason) => format!("waiting: {reason}"),
                };
                println!(
                    "relayer: chain {} job {} (order {}): {line}",
                    limits.chain_id, report.job_id, report.order_id
                );
            }
            Ok(None) => tokio::time::sleep(IDLE).await,
            Err(error) => {
                eprintln!("relayer: chain {}: database error: {error}", limits.chain_id);
                tokio::time::sleep(IDLE).await;
            }
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}
