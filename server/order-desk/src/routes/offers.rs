//! Providers' offers. The Matchmaker sends every open strategy's offer (`POST /internal/offers`)
//! every few seconds; each cross-chain quote goes to the cheapest offer that can cover the order,
//! counting what each provider has already been promised by orders still being paid.

use std::{
    collections::HashMap,
    sync::RwLock,
    time::{Duration, Instant},
};

use alloy::primitives::Address;
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use common::{
    amount::Amount,
    offers::{Offer, Offers, best_offer},
};

use super::{AppState, events::has_token};
use crate::error::ApiError;

/// Offers older than this are not quoted from: the Matchmaker sends them every 15 s, so older ones
/// mean it is down, and nobody would fill.
const FRESH_FOR: Duration = Duration::from_secs(90);

/// The latest offers from the Matchmaker.
#[derive(Default)]
pub struct OfferBook {
    latest: RwLock<Option<(Vec<Offer>, Instant)>>,
}

impl OfferBook {
    pub fn replace(&self, offers: Vec<Offer>) {
        *self.latest.write().expect("offer book") = Some((offers, Instant::now()));
    }

    /// The offers, if fresh; none otherwise.
    pub fn current(&self) -> Vec<Offer> {
        match self.latest.read().expect("offer book").as_ref() {
            Some((offers, at)) if at.elapsed() < FRESH_FOR => offers.clone(),
            _ => Vec::new(),
        }
    }
}

/// The provider a quote goes to, and the fee they ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chosen {
    pub provider: Address,
    pub fee_bps: u16,
}

pub async fn receive(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Offers>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    if !has_token(&headers, &state.internal_token) {
        return Err(ApiError::unauthorized());
    }
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_offers", rejection.body_text()))?;
    // The Matchmaker already keeps to the range; checked again so a quote never carries a fee
    // outside it.
    let fees = state.chains.provider_fee_bps_range;
    let offers = body.offers.into_iter().filter(|offer| fees.allows(offer.fee_bps)).collect();
    state.offers.replace(offers);
    Ok(StatusCode::NO_CONTENT)
}

/// The cheapest provider who can pay `amount` (USDT units) to the shop for `order_id`. What each
/// provider is owed by other orders still being paid (quoted with a live quote, being sent, locked,
/// or filled but not yet settled) is not free. A filled order also leaves the provider's offer
/// until the next one arrives, so it is counted twice for a few seconds: erring that way only sends
/// a quote to another provider, never to one who cannot pay. Callers hold `AppState::quoting`, so
/// two quotes cannot both count on the same free amount.
pub async fn choose(state: &AppState, order_id: &str, amount: Amount, now: i64) -> Result<Chosen, ApiError> {
    let offers = state.offers.current();
    if offers.is_empty() {
        return Err(ApiError::conflict(
            "crosschain_unavailable",
            "no provider can take payments from other chains right now; pay with USDT on Whitechain",
        ));
    }
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT o.provider, SUM(o.amount)::text FROM orders o
         WHERE o.provider IS NOT NULL AND o.id <> $1
           AND (o.status IN ('paying', 'source_locked', 'destination_locked')
                OR (o.status = 'quoted'
                    AND EXISTS (SELECT 1 FROM quotes q WHERE q.order_id = o.id AND q.expires_at > $2)))
         GROUP BY o.provider",
    )
    .bind(order_id)
    .bind(now)
    .fetch_all(&state.pool)
    .await?;
    let mut promised = HashMap::new();
    for (provider, total) in rows {
        let provider: Address = provider.parse().map_err(ApiError::internal)?;
        let total: Amount = total.parse().map_err(ApiError::internal)?;
        promised.insert(provider, total.0);
    }
    let offer = best_offer(&offers, amount.0, &promised).ok_or_else(|| {
        ApiError::conflict(
            "not_enough_liquidity",
            "no provider can cover this amount from another chain right now; pay with USDT on Whitechain or try a smaller amount",
        )
    })?;
    Ok(Chosen { provider: offer.provider, fee_bps: offer.fee_bps })
}
