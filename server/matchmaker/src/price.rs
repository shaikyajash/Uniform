//! The USD prices Bob values EURC and priced-token (WETH, WBTC, ...) source locks at, read from a
//! public price API (CoinGecko's simple price reply) and cached, one `CoinPrice` per coin.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::sync::Mutex;

/// A price is reused for this long, so the API's rate limit is never reached.
const CACHE_FOR: Duration = Duration::from_secs(60);
/// After a failed read, callers get the failure for this long instead of asking the API again.
const FAILURE_FOR: Duration = Duration::from_secs(30);
/// A price the API last updated longer ago than this is refused.
const MAX_AGE_SECONDS: u64 = 600;
/// EURC prices outside this range (in micro-dollars) are treated as a broken feed, not a market
/// move. EUR/USD has stayed inside it for over a decade.
pub const EURC_SANE_RANGE: std::ops::RangeInclusive<u64> = 900_000..=1_400_000;
/// Any other coin: only a price of zero (or below a micro-dollar) is refused.
pub const ANY_PRICE: std::ops::RangeInclusive<u64> = 1..=u64::MAX;

enum Cached {
    Price(Instant, u64),
    Failed(Instant, String),
}

/// One coin's price, by its CoinGecko id (`euro-coin`, `ethereum`, `bitcoin`).
pub struct CoinPrice {
    http: reqwest::Client,
    url: String,
    id: String,
    sane: std::ops::RangeInclusive<u64>,
    cached: Mutex<Option<Cached>>,
}

/// CoinGecko's simple price URL for `id`, from the API's `/simple/price` endpoint.
pub fn simple_price_url(api: &str, id: &str) -> String {
    format!("{api}?ids={id}&vs_currencies=usd&include_last_updated_at=true")
}

impl CoinPrice {
    /// `url` must reply `{"<id>": {"usd": ..., "last_updated_at": ...}}`; prices outside `sane`
    /// (micro-dollars) are refused.
    pub fn new(
        url: String,
        id: &str,
        sane: std::ops::RangeInclusive<u64>,
    ) -> Result<CoinPrice, String> {
        // The price API refuses requests that name no client.
        let http = reqwest::Client::builder()
            .user_agent(concat!("uniform-matchmaker/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(CoinPrice {
            http,
            url,
            id: id.to_string(),
            sane,
            cached: Mutex::new(None),
        })
    }

    /// One coin in USD micro-dollars (1.12 USD = 1_120_000).
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
            .map_err(|e| format!("{} price: {}", self.id, e.without_url()))?
            .json()
            .await
            .map_err(|e| format!("{} price: {}", self.id, e.without_url()))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        parse(&body, &self.id, &self.sane, now)
    }
}

/// Reads `{"euro-coin": {"usd": 1.12, "last_updated_at": 1793...}}` (for `id` = `euro-coin`) as
/// micro-dollars, refusing a price older than `MAX_AGE_SECONDS` at `now` or outside `sane`.
fn parse(
    body: &Value,
    id: &str,
    sane: &std::ops::RangeInclusive<u64>,
    now: u64,
) -> Result<u64, String> {
    let coin = &body[id];
    let usd = coin["usd"]
        .as_f64()
        .ok_or(format!("{id} price: no {id}.usd in the reply"))?;
    let updated = coin["last_updated_at"]
        .as_u64()
        .ok_or(format!("{id} price: no {id}.last_updated_at in the reply"))?;
    if now.saturating_sub(updated) > MAX_AGE_SECONDS {
        return Err(format!("{id} price: last updated {}s ago", now - updated));
    }
    let micro = (usd * 1_000_000.0).round();
    if !micro.is_finite()
        || micro < 0.0
        || micro >= u64::MAX as f64
        || !sane.contains(&(micro as u64))
    {
        return Err(format!("{id} price: {usd} USD is outside the sane range"));
    }
    Ok(micro as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: u64 = 1_800_000_000;

    fn eurc(body: &Value, now: u64) -> Result<u64, String> {
        parse(body, "euro-coin", &EURC_SANE_RANGE, now)
    }

    fn reply(usd: Value) -> Value {
        json!({"euro-coin": {"usd": usd, "last_updated_at": NOW - 30}})
    }

    #[test]
    fn reads_the_price_in_micro_dollars() {
        assert_eq!(eurc(&reply(json!(1.12)), NOW), Ok(1_120_000));
        assert_eq!(eurc(&reply(json!(1.0834567)), NOW), Ok(1_083_457));
    }

    #[test]
    fn the_sane_range_edges_are_accepted() {
        assert_eq!(eurc(&reply(json!(0.9)), NOW), Ok(900_000));
        assert_eq!(eurc(&reply(json!(1.4)), NOW), Ok(1_400_000));
        assert!(eurc(&reply(json!(0.8999994)), NOW).is_err());
        assert!(eurc(&reply(json!(1.4000006)), NOW).is_err());
    }

    #[test]
    fn refuses_missing_or_absurd_prices() {
        assert!(eurc(&json!({}), NOW).is_err());
        for usd in [
            json!("1.12"),
            json!(0.0),
            json!(-1.12),
            json!(1.6),
            json!(1e300),
        ] {
            assert!(eurc(&reply(usd.clone()), NOW).is_err(), "{usd}");
        }
    }

    #[test]
    fn refuses_a_stale_or_undated_price() {
        let at = |updated: u64| json!({"euro-coin": {"usd": 1.12, "last_updated_at": updated}});
        assert_eq!(eurc(&at(NOW - 600), NOW), Ok(1_120_000));
        assert!(eurc(&at(NOW - 601), NOW).is_err());
        assert!(eurc(&json!({"euro-coin": {"usd": 1.12}}), NOW).is_err());
    }

    #[test]
    fn any_coin_is_read_by_its_id_without_the_eurc_range() {
        let btc = json!({"bitcoin": {"usd": 97_123.456789, "last_updated_at": NOW - 30}});
        assert_eq!(parse(&btc, "bitcoin", &ANY_PRICE, NOW), Ok(97_123_456_789));
        assert!(parse(&btc, "ethereum", &ANY_PRICE, NOW).is_err());
        assert!(parse(&btc, "bitcoin", &EURC_SANE_RANGE, NOW).is_err());
        for usd in [json!(0.0), json!(0.0000004), json!(-3000.0), json!(1e300)] {
            let body = json!({"ethereum": {"usd": usd, "last_updated_at": NOW - 30}});
            assert!(parse(&body, "ethereum", &ANY_PRICE, NOW).is_err(), "{usd}");
        }
    }

    #[test]
    fn the_url_names_the_coin() {
        assert_eq!(
            simple_price_url("https://api.coingecko.com/api/v3/simple/price", "bitcoin"),
            "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd&include_last_updated_at=true"
        );
    }
}
