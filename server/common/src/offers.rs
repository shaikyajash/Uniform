//! Providers' offers: what each open strategy can pay out now and at what fee. The Matchmaker reads
//! them from Aqua and sends them to the Order Desk (`POST /internal/offers`); the Order Desk gives
//! each cross-chain quote to the cheapest offer that covers it.

use std::collections::HashMap;

use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};

use crate::amount::Amount;

/// One open strategy, as the Matchmaker reads it. Amounts are USDT units (6 decimals).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    /// The provider: the strategy's maker, and the receiver of the customer's lock.
    pub provider: Address,
    pub fee_bps: u16,
    pub max_per_order: Amount,
    /// What the pool can take now: the strategy's balance, capped by the provider's wallet and
    /// approval to Aqua.
    pub available: Amount,
}

/// Every offer at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offers {
    pub offers: Vec<Offer>,
}

/// The offer to take an order of `amount`: among those that allow it per order and still have it
/// free after what their provider has already been promised (`promised`, by provider), the lowest
/// fee; of equal fees, the most free. A provider's offers share one wallet, so what is promised to
/// them counts against each.
pub fn best_offer<'a>(offers: &'a [Offer], amount: U256, promised: &HashMap<Address, U256>) -> Option<&'a Offer> {
    let free = |offer: &Offer| {
        offer.available.0.saturating_sub(promised.get(&offer.provider).copied().unwrap_or_default())
    };
    offers
        .iter()
        .filter(|offer| offer.max_per_order.0 >= amount && free(offer) >= amount)
        .min_by(|a, b| a.fee_bps.cmp(&b.fee_bps).then(free(b).cmp(&free(a))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const ASHA: Address = address!("00000000000000000000000000000000000000a1");
    const BEN: Address = address!("00000000000000000000000000000000000000b2");

    fn offer(provider: Address, fee_bps: u16, available: u64) -> Offer {
        Offer { provider, fee_bps, max_per_order: Amount::from(100_000_000u64), available: Amount::from(available) }
    }

    #[test]
    fn the_cheapest_offer_that_covers_the_order_wins() {
        let offers = [offer(ASHA, 30, 50_000_000), offer(BEN, 10, 5_000_000)];
        let none = HashMap::new();
        assert_eq!(best_offer(&offers, U256::from(2_000_000), &none), Some(&offers[1]));
        // Ben cannot cover 8 USDT, so Asha takes it at her higher fee.
        assert_eq!(best_offer(&offers, U256::from(8_000_000), &none), Some(&offers[0]));
        assert_eq!(best_offer(&offers, U256::from(60_000_000), &none), None);
    }

    #[test]
    fn equal_fees_go_to_the_most_free() {
        let offers = [offer(ASHA, 20, 5_000_000), offer(BEN, 20, 9_000_000)];
        assert_eq!(best_offer(&offers, U256::from(1_000_000), &HashMap::new()), Some(&offers[1]));
    }

    #[test]
    fn what_is_already_promised_is_not_free() {
        let offers = [offer(ASHA, 30, 50_000_000), offer(BEN, 10, 5_000_000)];
        let promised = HashMap::from([(BEN, U256::from(4_000_000))]);
        // Ben has 1 USDT left free, so a 2 USDT order goes to Asha.
        assert_eq!(best_offer(&offers, U256::from(2_000_000), &promised), Some(&offers[0]));
        assert_eq!(best_offer(&offers, U256::from(1_000_000), &promised), Some(&offers[1]));
    }

    #[test]
    fn the_per_order_limit_applies() {
        let mut small = offer(BEN, 10, 50_000_000);
        small.max_per_order = Amount::from(1_000_000u64);
        let offers = [small, offer(ASHA, 30, 50_000_000)];
        assert_eq!(best_offer(&offers, U256::from(2_000_000), &HashMap::new()), Some(&offers[1]));
    }

    #[test]
    fn offers_are_camel_case_json_with_decimal_amounts() {
        let json = serde_json::to_value(Offers { offers: vec![offer(ASHA, 30, 7)] }).unwrap();
        assert_eq!(json["offers"][0]["feeBps"], 30);
        assert_eq!(json["offers"][0]["available"], "7");
        assert_eq!(json["offers"][0]["maxPerOrder"], "100000000");
    }
}
