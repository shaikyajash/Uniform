//! Settings from the environment and `shared/chains.json`.

use std::{collections::HashMap, env, str::FromStr};

use alloy::primitives::B256;
use common::chains::Chains;

use crate::rules::{Provider, Timing};

/// Ethereum itself (mainnet and Sepolia), as opposed to the cheaper rollups.
const ETHEREUM_CHAINS: [u64; 2] = [1, 11_155_111];

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
    /// Skip filling when the filler holds less than this on the source chain, so it can always
    /// afford the source redeem. Per customer chain: `MIN_FILLER_GAS_WEI_<chainId>`, else 0.002
    /// ETH on Ethereum (mainnet, Sepolia), where a redeem costs far more, and `MIN_FILLER_GAS_WEI`
    /// (default 0.0001 ETH) elsewhere.
    pub min_filler_gas_wei: HashMap<u64, u128>,
    /// Where one EURC's USD price is read (CoinGecko's reply shape).
    pub eurc_price_url: String,
    /// CoinGecko's simple price endpoint, for tokens with a `priceId` in chains.json (WETH, WBTC).
    pub price_api_url: String,
    /// EURC and priced-token locks are valued this far below the price, so a price move before Bob
    /// converts the token cannot leave him short (100 = 1%). `PRICE_BUFFER_BPS`, else the older
    /// `EURC_PRICE_BUFFER_BPS`. Keep it below the Order Desk's quote buffer.
    pub price_buffer_bps: u16,
    /// Customer chains listed here read up to their "safe" block: the strongest guard against a
    /// reorg, but on Base it trails the head by 50 s to 2 min. Default: none, so fills are fast.
    /// On Ethereum "safe" trails by about 9 minutes, too long for a 20-minute lock.
    pub safe_tag_chains: Vec<u64>,
    /// Blocks to wait before trusting a log, per chain: `CONFIRMATIONS_<chainId>`, else
    /// `confirmations` in chains.json.
    pub confirmations: HashMap<u64, u64>,
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
        let config = Config {
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
                min_gap: number("MIN_GAP_SECONDS", 300)?,
                min_destination_lock: 300,
            },
            db_path: env::var("DB_PATH").unwrap_or_else(|_| "matchmaker.db".into()),
            poll_seconds: number("POLL_SECONDS", 2)?,
            max_block_range: number("MAX_BLOCK_RANGE", 1000)?,
            min_filler_gas_wei: HashMap::new(),
            eurc_price_url: env::var("EURC_PRICE_URL").unwrap_or_else(|_| {
                "https://api.coingecko.com/api/v3/simple/price?ids=euro-coin&vs_currencies=usd&include_last_updated_at=true"
                    .into()
            }),
            price_api_url: env::var("PRICE_API_URL").unwrap_or_else(|_| {
                "https://api.coingecko.com/api/v3/simple/price".into()
            }),
            price_buffer_bps: match env::var("PRICE_BUFFER_BPS") {
                Ok(_) => number("PRICE_BUFFER_BPS", 100)?,
                Err(_) => number("EURC_PRICE_BUFFER_BPS", 100)?,
            },
            safe_tag_chains: chain_list("SAFE_TAG_CHAINS", &[])?,
            confirmations: HashMap::new(),
            chains,
        };
        let mut config = config;
        for chain in config.chains.by_name.values() {
            let wait = number(
                &format!("CONFIRMATIONS_{}", chain.chain_id),
                chain.confirmations,
            )?;
            config.confirmations.insert(chain.chain_id, wait);
        }
        let rollup_gas = number("MIN_FILLER_GAS_WEI", 100_000_000_000_000u128)?;
        for chain in config.chains.customer_chains() {
            let default = if ETHEREUM_CHAINS.contains(&chain.chain_id) {
                2_000_000_000_000_000
            } else {
                rollup_gas
            };
            let min = number(&format!("MIN_FILLER_GAS_WEI_{}", chain.chain_id), default)?;
            config.min_filler_gas_wei.insert(chain.chain_id, min);
        }
        config.check()?;
        Ok(config)
    }

    /// Rejects settings that would make the Matchmaker stall or fill unsafely.
    fn check(&self) -> Result<(), String> {
        if self.internal_token.trim().is_empty() {
            return Err("INTERNAL_EVENTS_TOKEN is empty".into());
        }
        // The shop address comes from the Order Desk, so the connection to it must not be open to
        // tampering unless it is on this machine.
        let local = ["http://127.0.0.1", "http://localhost", "http://[::1]"]
            .iter()
            .any(|p| self.order_desk_url.starts_with(p));
        if !self.order_desk_url.starts_with("https://") && !local {
            return Err("ORDER_DESK_URL must use https unless it is on this machine".into());
        }
        if self.max_block_range == 0 {
            return Err("MAX_BLOCK_RANGE must be at least 1".into());
        }
        if self.poll_seconds == 0 {
            return Err("POLL_SECONDS must be at least 1".into());
        }
        if self.provider.max_per_order.is_zero() {
            return Err("STRATEGY_MAX_PER_ORDER must be above zero".into());
        }
        if self.provider.fee_bps != self.chains.provider_fee_bps {
            return Err(format!(
                "STRATEGY_FEE_BPS ({}) differs from providerFeeBps in chains.json ({}): quotes and fills would disagree",
                self.provider.fee_bps, self.chains.provider_fee_bps
            ));
        }
        if self.price_buffer_bps > 1_000 {
            return Err(format!(
                "PRICE_BUFFER_BPS ({}) must be at most 1000 (10%)",
                self.price_buffer_bps
            ));
        }
        if !self.eurc_price_url.starts_with("https://") {
            return Err("EURC_PRICE_URL must use https".into());
        }
        if !self.price_api_url.starts_with("https://") {
            return Err("PRICE_API_URL must use https".into());
        }
        let customers: Vec<u64> = self
            .chains
            .customer_chains()
            .map(|c| c.chain_id)
            .collect();
        if let Some(id) = self
            .safe_tag_chains
            .iter()
            .find(|id| !customers.contains(id))
        {
            return Err(format!(
                "SAFE_TAG_CHAINS lists {id}, which is not a customer chain in chains.json"
            ));
        }
        let t = self.timing;
        if t.min_gap < 300 {
            return Err(format!(
                "MIN_GAP_SECONDS ({}) must be at least 300 (time to redeem after the shop is paid)",
                t.min_gap
            ));
        }
        if !(300..=7200).contains(&t.destination_lock) {
            return Err(format!(
                "timeoutsSeconds.destinationLock ({}) must be between 300 and 7200, the DestinationLock limits",
                t.destination_lock
            ));
        }
        if t.destination_lock
            .checked_add(t.min_gap)
            .is_none_or(|needed| t.fill_cutoff < needed)
        {
            return Err(format!(
                "fillCutoff ({}) must be at least destinationLock ({}) + MIN_GAP_SECONDS ({})",
                t.fill_cutoff, t.destination_lock, t.min_gap
            ));
        }
        Ok(())
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

/// A comma-separated list of chain ids, or `default` when unset.
fn chain_list(name: &str, default: &[u64]) -> Result<Vec<u64>, String> {
    match env::var(name) {
        Ok(value) => parse_chain_list(&value).map_err(|e| format!("{name}: {e}")),
        Err(_) => Ok(default.to_vec()),
    }
}

fn parse_chain_list(value: &str) -> Result<Vec<u64>, String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.parse().map_err(|_| format!("{v} is not a chain id")))
        .collect()
}

fn number<T: FromStr>(name: &str, default: T) -> Result<T, String> {
    match env::var(name) {
        Ok(value) => value.parse().map_err(|_| format!("{name} is not a number")),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_lists_are_comma_separated_ids() {
        assert_eq!(parse_chain_list("84532"), Ok(vec![84532]));
        assert_eq!(parse_chain_list(" 84532, 8453 ,"), Ok(vec![84532, 8453]));
        assert_eq!(parse_chain_list(""), Ok(vec![]));
        assert!(parse_chain_list("base").is_err());
    }
}
