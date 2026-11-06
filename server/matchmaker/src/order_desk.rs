//! The Order Desk over HTTP: read an order, and report chain events.

use alloy::primitives::{Address, B256, U256};
use common::{amount::Amount, status::OrderStatus};
use serde::Deserialize;

use crate::rules::OrderView;

const USDT_DECIMALS: u8 = 6;

pub struct OrderDesk {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

#[derive(Deserialize)]
struct OrderJson {
    merchant: ShopJson,
    amount: String,
    status: OrderStatus,
    /// Empty until the customer has signed a cross-chain quote.
    #[serde(default)]
    legs: Vec<LegJson>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegJson {
    chain_id: u64,
    lock_id: B256,
    expiry: u64,
}

/// One source lock the customer signed for. Its id commits to the order, Bob, the hashlock, the
/// expiry, the token and the amount, so a lock on chain under this id is exactly what was signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct OrderLeg {
    pub chain_id: u64,
    pub lock_id: B256,
    pub expiry: u64,
}

/// An order and the source locks of its latest signed quote: one, or several for a split payment.
/// No legs means nothing has been signed yet, so no lock pays for it.
#[derive(Debug, Clone)]
pub struct Order {
    pub view: OrderView,
    pub legs: Vec<OrderLeg>,
}

#[derive(Deserialize)]
struct ShopJson {
    address: Address,
}

#[derive(Debug)]
pub enum OrderLookup {
    Found(Order),
    NotFound,
}

impl OrderDesk {
    pub fn new(base_url: String, token: String) -> OrderDesk {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("http client");
        OrderDesk {
            http,
            base_url,
            token,
        }
    }

    pub async fn order(&self, order_id: B256) -> Result<OrderLookup, String> {
        let url = format!("{}/v1/orders/{}", self.base_url, order_id);
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(OrderLookup::NotFound);
        }
        if !response.status().is_success() {
            return Err(format!("GET {url}: {}", response.status()));
        }
        let order: OrderJson = response.json().await.map_err(|e| e.to_string())?;
        let amount =
            Amount::from_decimal(&order.amount, USDT_DECIMALS).map_err(|e| e.to_string())?;
        Ok(OrderLookup::Found(Order {
            view: OrderView {
                merchant: order.merchant.address,
                amount: U256::from(amount.0),
                status: order.status,
            },
            legs: order
                .legs
                .into_iter()
                .map(|l| OrderLeg {
                    chain_id: l.chain_id,
                    lock_id: l.lock_id,
                    expiry: l.expiry,
                })
                .collect(),
        }))
    }

    /// Sends one event (the JSON of `common::events::ChainEvent`). `Ok` means the Order Desk has
    /// it, including when it was already reported. `Rejected` means it refused this event for good
    /// (a 4xx other than auth or rate limiting), so retrying will not help.
    pub async fn report(&self, body: &str) -> Result<(), ReportError> {
        let response = self
            .http
            .post(format!("{}/internal/events", self.base_url))
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| ReportError::Retry(e.without_url().to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        let permanent =
            status.is_client_error() && !matches!(status.as_u16(), 401 | 403 | 408 | 429);
        if permanent {
            Err(ReportError::Rejected(format!("{status}: {text}")))
        } else {
            Err(ReportError::Retry(format!("{status}: {text}")))
        }
    }
}

#[derive(Debug)]
pub enum ReportError {
    Retry(String),
    Rejected(String),
}
