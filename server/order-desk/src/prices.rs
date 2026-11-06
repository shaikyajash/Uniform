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

pub struct Prices {
    /// `None`: only `fixed` prices (tests).
    http: Option<reqwest::Client>,
    fixed: HashMap<String, u64>,
    cache: Mutex<HashMap<String, (u64, Instant)>>,
}

impl Prices {
    pub fn coingecko() -> Prices {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(8)).build().expect("http client");
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
            .get(format!("{COINGECKO}?ids={id}&vs_currencies=usd"))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| format!("price of {id}: {}", error.without_url()))?
            .json()
            .await
            .map_err(|error| format!("price of {id}: {error}"))?;
        let usd = reply[id]["usd"].as_f64().filter(|usd| usd.is_finite() && *usd > 0.0).ok_or_else(|| format!("no USD price for {id}"))?;
        let price = (usd * 1_000_000.0).round() as u64;
        self.cache.lock().expect("price cache").insert(id.to_owned(), (price, Instant::now()));
        Ok(price)
    }
}
