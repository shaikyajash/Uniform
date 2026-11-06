//! Settings from the environment and `shared/chains.json`.

use std::{env, str::FromStr};

use alloy::primitives::B256;
use common::chains::Chains;

use crate::rules::{Provider, Timing};

pub struct Config {
    pub chains: Chains,
    pub order_desk_url: String,
    pub internal_token: String,
    /// The Matchmaker's own key: pays gas on every chain and is the strategy's filler.
    pub filler_key: String,
    pub provider: Provider,
    pub strategy_salt: B256,
    pub timing: Timing,
    pub db_path: String,
    pub poll_seconds: u64,
    /// Largest block range read in one `eth_getLogs` call.
    pub max_block_range: u64,
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        let chains_path =
            env::var("CHAINS_PATH").unwrap_or_else(|_| "../shared/chains.json".into());
        let chains = Chains::load(&chains_path).map_err(|e| e.to_string())?;
        let timeouts = chains.timeouts_seconds;
        let provider = Provider {
            address: parse("BOB_ADDRESS")?,
            max_per_order: parse("STRATEGY_MAX_PER_ORDER")?,
            fee_bps: env::var("STRATEGY_FEE_BPS")
                .map(|v| {
                    v.parse()
                        .map_err(|_| "STRATEGY_FEE_BPS is not a number".to_string())
                })
                .unwrap_or(Ok(chains.provider_fee_bps))?,
        };
        Ok(Config {
            order_desk_url: required("ORDER_DESK_URL")?
                .trim_end_matches('/')
                .to_string(),
            internal_token: required("INTERNAL_EVENTS_TOKEN")?,
            filler_key: required("FILLER_PRIVATE_KEY")?,
            provider,
            strategy_salt: parse("STRATEGY_SALT")?,
            timing: Timing {
                fill_cutoff: timeouts.fill_cutoff,
                destination_lock: timeouts.destination_lock,
                min_gap: 300,
                min_destination_lock: 300,
            },
            db_path: env::var("DB_PATH").unwrap_or_else(|_| "matchmaker.db".into()),
            poll_seconds: env::var("POLL_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4),
            max_block_range: env::var("MAX_BLOCK_RANGE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000),
            chains,
        })
    }

    /// `RPC_<chainId>` overrides the RPC in chains.json (for example a private Whitechain RPC).
    pub fn rpc_for(&self, chain_id: u64, default: &str) -> String {
        env::var(format!("RPC_{chain_id}")).unwrap_or_else(|_| default.to_string())
    }
}

fn required(name: &str) -> Result<String, String> {
    env::var(name).map_err(|_| format!("{name} is not set"))
}

fn parse<T: FromStr>(name: &str) -> Result<T, String> {
    required(name)?
        .parse()
        .map_err(|_| format!("{name} is not valid"))
}
