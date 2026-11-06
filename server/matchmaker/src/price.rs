//! The EURC price Bob values EURC source locks at, read from a public price API and cached.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::sync::Mutex;

/// A price is reused for this long, so the API's rate limit is never reached.
const CACHE_FOR: Duration = Duration::from_secs(60);
/// After a failed read, callers get the failure for this long instead of asking the API again.
const FAILURE_FOR: Duration = Duration::from_secs(30);
/// A price the API last updated longer ago than this is refused.
const MAX_AGE_SECONDS: u64 = 600;
/// Prices outside this range (in micro-dollars) are treated as a broken feed, not a market move.
/// EUR/USD has stayed inside it for over a decade.
const SANE_RANGE: std::ops::RangeInclusive<u64> = 900_000..=1_400_000;

enum Cached {
    Price(Instant, u64),
    Failed(Instant, String),
}

pub struct EurcPrice {
    http: reqwest::Client,
    url: String,
    cached: Mutex<Option<Cached>>,
}

impl EurcPrice {
    pub fn new(url: String) -> Result<EurcPrice, String> {
        // The price API refuses requests that name no client.
        let http = reqwest::Client::builder()
            .user_agent(concat!("uniform-matchmaker/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(EurcPrice {
            http,
            url,
            cached: Mutex::new(None),
        })
    }

    /// One EURC in USD micro-dollars (1.12 USD = 1_120_000).
    pub async fn micro_usd(&self) -> Result<u64, String> {
        let mut cached = self.cached.lock().await;
        match &*cached {
            Some(Cached::Price(at, price)) if at.elapsed() < CACHE_FOR => return Ok(*price),
            Some(Cached::Failed(at, e)) if at.elapsed() < FAILURE_FOR => return Err(e.clone()),
            _ => {}
        }
        let result = self.fetch().await;
        *cached = Some(match &result {
            Ok(price) => Cached::Price(Instant::now(), *price),
            Err(e) => Cached::Failed(Instant::now(), e.clone()),
        });
        result
    }

    async fn fetch(&self) -> Result<u64, String> {
        let body: Value = self
            .http
            .get(&self.url)
            .header("accept", "application/json")
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("EURC price: {}", e.without_url()))?
            .json()
            .await
            .map_err(|e| format!("EURC price: {}", e.without_url()))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        parse(&body, now)
    }
}

/// Reads `{"euro-coin": {"usd": 1.12, "last_updated_at": 1793...}}` as micro-dollars, refusing a
/// price older than `MAX_AGE_SECONDS` at `now` or outside `SANE_RANGE`.
fn parse(body: &Value, now: u64) -> Result<u64, String> {
    let coin = &body["euro-coin"];
    let usd = coin["usd"]
        .as_f64()
        .ok_or("EURC price: no euro-coin.usd in the reply")?;
    let updated = coin["last_updated_at"]
        .as_u64()
        .ok_or("EURC price: no euro-coin.last_updated_at in the reply")?;
    if now.saturating_sub(updated) > MAX_AGE_SECONDS {
        return Err(format!("EURC price: last updated {}s ago", now - updated));
    }
    let micro = (usd * 1_000_000.0).round();
    if !micro.is_finite() || !SANE_RANGE.contains(&(micro as u64)) || micro < 0.0 {
        return Err(format!("EURC price: {usd} USD is outside the sane range"));
    }
    Ok(micro as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: u64 = 1_800_000_000;

    fn reply(usd: Value) -> Value {
        json!({"euro-coin": {"usd": usd, "last_updated_at": NOW - 30}})
    }

    #[test]
    fn reads_the_price_in_micro_dollars() {
        assert_eq!(parse(&reply(json!(1.12)), NOW), Ok(1_120_000));
        assert_eq!(parse(&reply(json!(1.0834567)), NOW), Ok(1_083_457));
    }

    #[test]
    fn the_sane_range_edges_are_accepted() {
        assert_eq!(parse(&reply(json!(0.9)), NOW), Ok(900_000));
        assert_eq!(parse(&reply(json!(1.4)), NOW), Ok(1_400_000));
        assert!(parse(&reply(json!(0.8999994)), NOW).is_err());
        assert!(parse(&reply(json!(1.4000006)), NOW).is_err());
    }

    #[test]
    fn refuses_missing_or_absurd_prices() {
        assert!(parse(&json!({}), NOW).is_err());
        for usd in [
            json!("1.12"),
            json!(0.0),
            json!(-1.12),
            json!(1.6),
            json!(1e300),
        ] {
            assert!(parse(&reply(usd.clone()), NOW).is_err(), "{usd}");
        }
    }

    #[test]
    fn refuses_a_stale_or_undated_price() {
        let at = |updated: u64| json!({"euro-coin": {"usd": 1.12, "last_updated_at": updated}});
        assert_eq!(parse(&at(NOW - 600), NOW), Ok(1_120_000));
        assert!(parse(&at(NOW - 601), NOW).is_err());
        assert!(parse(&json!({"euro-coin": {"usd": 1.12}}), NOW).is_err());
    }
}
