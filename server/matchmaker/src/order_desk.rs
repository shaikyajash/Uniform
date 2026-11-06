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
    shop: ShopJson,
    amount: String,
    status: OrderStatus,
}

#[derive(Deserialize)]
struct ShopJson {
    address: Address,
}

#[derive(Debug)]
pub enum OrderLookup {
    Found(OrderView),
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
        Ok(OrderLookup::Found(OrderView {
            merchant: order.shop.address,
            amount: U256::from(amount.0),
            status: order.status,
        }))
    }

    /// Sends one event (the JSON of `common::events::ChainEvent`). Ok means the Order Desk has it,
    /// including when it was already reported.
    pub async fn report(&self, body: &str) -> Result<(), String> {
        let response = self
            .http
            .post(format!("{}/internal/events", self.base_url))
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!(
                "{}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            ))
        }
    }
}
