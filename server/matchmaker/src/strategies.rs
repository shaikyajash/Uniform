//! Providers' liquidity, as they manage it on the Earn page: every strategy anyone shipped to the
//! PayoutPool for Uniform (USDT, our filler, a fee within the allowed range). Whoever ships one is a
//! provider; their fee is their own. Aqua's events cannot be filtered by maker
//! on a node, so the strategies are found through the Whitechain explorer, which returns Aqua's
//! logs decoded. Nothing found there is trusted as is: a strategy's bytes must hash to the hash
//! Aqua logged, and its balance and state are read from Aqua itself before every fill.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use alloy::{
    primitives::{Address, B256, Bytes, keccak256},
    sol_types::SolValue,
};
use bindings::PayoutPool;
use common::chains::FeeRange;
use serde::Deserialize;

/// How long a list of strategies is reused before the explorer is asked again: a strategy shipped
/// on the Earn page is used within this long.
const FRESH_FOR: Duration = Duration::from_secs(30);
/// The explorer pages Aqua's logs (50 a page), newest first; every provider's strategies must be
/// within this many pages to be filled from.
const MAX_PAGES: usize = 50;

/// What makes a strategy one the Matchmaker may fill from.
#[derive(Debug, Clone, Copy)]
pub struct Wanted {
    pub token: Address,
    pub filler: Address,
    pub fees: FeeRange,
}

pub struct Strategies {
    http: reqwest::Client,
    /// `<explorer>/api/v2/addresses/<aqua>/logs`
    logs_url: String,
    pool: Address,
    wanted: Wanted,
    cache: Mutex<Option<(Vec<PayoutPool::Strategy>, Instant)>>,
}

#[derive(Deserialize)]
struct Page {
    items: Vec<Log>,
    next_page_params: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Deserialize)]
struct Log {
    decoded: Option<Decoded>,
}

#[derive(Deserialize)]
struct Decoded {
    method_call: String,
    parameters: Vec<Param>,
}

#[derive(Deserialize)]
struct Param {
    name: String,
    value: serde_json::Value,
}

impl Strategies {
    pub fn new(explorer: &str, aqua: Address, pool: Address, wanted: Wanted) -> Result<Strategies, String> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("uniform-matchmaker/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        let logs_url = format!("{}/api/v2/addresses/{aqua:#x}/logs", explorer.trim_end_matches('/'));
        Ok(Strategies { http, logs_url, pool, wanted, cache: Mutex::new(None) })
    }

    /// Every strategy shipped for Uniform, by any provider, oldest first. Docked or empty ones are included;
    /// the caller reads their state on chain.
    pub async fn shipped(&self) -> Result<Vec<PayoutPool::Strategy>, String> {
        if let Some((list, at)) = self.cache.lock().expect("strategy cache").as_ref()
            && at.elapsed() < FRESH_FOR
        {
            return Ok(list.clone());
        }
        let mut found = Vec::new();
        let mut query = String::new();
        for _ in 0..MAX_PAGES {
            let page: Page = self
                .http
                .get(format!("{}{query}", self.logs_url))
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| format!("explorer: {}", e.without_url()))?
                .json()
                .await
                .map_err(|e| format!("explorer: {e}"))?;
            found.extend(page.items.iter().filter_map(|log| self.strategy_of(log)));
            let Some(next) = page.next_page_params else { break };
            query = format!(
                "?{}",
                next.iter()
                    .map(|(k, v)| format!("{k}={}", v.as_str().map_or_else(|| v.to_string(), str::to_owned)))
                    .collect::<Vec<_>>()
                    .join("&")
            );
        }
        // The explorer lists newest first.
        found.reverse();
        found.dedup_by_key(|s| hash_of(s));
        *self.cache.lock().expect("strategy cache") = Some((found.clone(), Instant::now()));
        Ok(found)
    }

    /// A `Shipped(maker, app, strategyHash, strategy)` log of a strategy we may fill from.
    fn strategy_of(&self, log: &Log) -> Option<PayoutPool::Strategy> {
        let decoded = log.decoded.as_ref()?;
        if !decoded.method_call.starts_with("Shipped(") {
            return None;
        }
        let param = |name: &str| decoded.parameters.iter().find(|p| p.name == name)?.value.as_str();
        let maker: Address = param("maker")?.parse().ok()?;
        let app: Address = param("app")?.parse().ok()?;
        let hash: B256 = param("strategyHash")?.parse().ok()?;
        let bytes: Bytes = param("strategy")?.parse().ok()?;
        if app != self.pool || keccak256(&bytes) != hash {
            return None;
        }
        let strategy = PayoutPool::Strategy::abi_decode(&bytes).ok()?;
        let ours = strategy.maker == maker
            && strategy.token == self.wanted.token
            && strategy.filler == self.wanted.filler
            && self.wanted.fees.allows(strategy.feeBps);
        ours.then_some(strategy)
    }
}

/// Aqua's id for a strategy: the hash of its ABI encoding, as the PayoutPool computes it.
pub fn hash_of(strategy: &PayoutPool::Strategy) -> B256 {
    keccak256(strategy.abi_encode())
}
