//! USD prices of tokens customers pay with through a Calibur batch (WETH, WBTC, ...), to quote how
//! much of the token covers an order. From CoinGecko's simple price API, kept for a short while so
//! quotes stay quick and the free API is not hammered.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde_json::Value;

const COINGECKO: &str = "https://api.coingecko.com/api/v3/simple/price";
/// How long a price is reused before it is fetched again.
const FRESH_FOR: Duration = Duration::from_secs(30);
/// CoinGecko's own update time: a price older than this is not quoted (the Matchmaker's limit too).
const MAX_AGE_SECONDS: u64 = 600;

pub struct Prices {
    /// `None`: only `fixed` prices (tests).
    http: Option<reqwest::Client>,
    fixed: HashMap<String, u64>,
    cache: Mutex<HashMap<String, (u64, Instant)>>,
}

impl Prices {
    pub fn coingecko() -> Prices {
        // CoinGecko refuses requests without a User-Agent.
        let http = reqwest::Client::builder()
            .user_agent(concat!("uniform-order-desk/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(8))
            .build()
            .expect("http client");
        Prices { http: Some(http), fixed: HashMap::new(), cache: Mutex::new(HashMap::new()) }
    }

    /// Prices that never change, in micro-USD per whole token.
    pub fn fixed(prices: &[(&str, u64)]) -> Prices {
        let fixed = prices.iter().map(|(id, price)| ((*id).to_owned(), *price)).collect();
        Prices { http: None, fixed, cache: Mutex::new(HashMap::new()) }
    }

    /// The price of one whole token in micro-USD (6 decimals), by CoinGecko id.
    pub async fn micro_usd(&self, id: &str) -> Result<u64, String> {
        if let Some(price) = self.fixed.get(id) {
            return Ok(*price);
        }
        if let Some((price, at)) = self.cache.lock().expect("price cache").get(id)
            && at.elapsed() < FRESH_FOR
        {
            return Ok(*price);
        }
        let http = self.http.as_ref().ok_or_else(|| format!("no price for {id}"))?;
        let reply: Value = http
            .get(format!("{COINGECKO}?ids={id}&vs_currencies=usd&include_last_updated_at=true"))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| format!("price of {id}: {}", error.without_url()))?
            .json()
            .await
            .map_err(|error| format!("price of {id}: {error}"))?;
        let usd = reply[id]["usd"].as_f64().filter(|usd| usd.is_finite() && *usd > 0.0).ok_or_else(|| format!("no USD price for {id}"))?;
        let updated = reply[id]["last_updated_at"].as_u64().ok_or_else(|| format!("no update time for {id}"))?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        if now.saturating_sub(updated) > MAX_AGE_SECONDS {
            return Err(format!("the price of {id} is {}s old", now.saturating_sub(updated)));
        }
        let price = (usd * 1_000_000.0).round() as u64;
        self.cache.lock().expect("price cache").insert(id.to_owned(), (price, Instant::now()));
        Ok(price)
    }
}
