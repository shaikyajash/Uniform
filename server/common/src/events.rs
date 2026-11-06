//! Chain events the Matchmaker reports to the Order Desk on `POST /internal/events`.
//!
//! This is the source of truth (docs/SPEC.md); `ChainEvent` in `shared/types.ts` mirrors it.

use alloy::primitives::{Address, B256};
use serde::{Deserialize, Serialize};

use crate::amount::Amount;

/// One contract event: where it happened and what it said.
///
/// The JSON is flat: `{"chainId", "blockNumber", "txHash", "logIndex", "type", ...fields}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainEvent {
    pub chain_id: u64,
    pub block_number: u64,
    pub tx_hash: B256,
    /// Position of the log in its block. Chain, transaction and log index together identify
    /// the event, so a report sent twice is stored once.
    pub log_index: u64,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// What the event said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum EventKind {
    /// DirectPay: a Path A payment moved USDT from the customer to the shop.
    Paid {
        payment_id: B256,
        order_id: B256,
        payer: Address,
        amount: Amount,
    },
    /// DirectPay: the shop sent part or all of a Path A payment back.
    DirectRefunded {
        payment_id: B256,
        order_id: B256,
        amount: Amount,
        total_refunded: Amount,
    },
    /// SourceLock: a customer leg is locked for Bob.
    Locked {
        lock_id: B256,
        order_id: B256,
        usdc_amount: Amount,
    },
    /// PayoutPool: Bob's USDT is locked for the shop in the DestinationLock.
    Filled {
        order_id: B256,
        destination_lock_id: B256,
        amount: Amount,
    },
    /// DestinationLock: the shop was paid (the onchain receipt).
    Settled {
        order_id: B256,
        amount: Amount,
    },
    /// SourceLock: Bob redeemed a customer leg with the public secret.
    Claimed { lock_id: B256 },
    /// SourceLock or DestinationLock (told apart by `chain_id`): a lock timed out and went back.
    Refunded { lock_id: B256 },
}

impl EventKind {
    /// The event's name, as stored in `events.name` and sent as `"type"`.
    pub const fn name(&self) -> &'static str {
        match self {
            EventKind::Paid { .. } => "Paid",
            EventKind::DirectRefunded { .. } => "DirectRefunded",
            EventKind::Locked { .. } => "Locked",
            EventKind::Filled { .. } => "Filled",
            EventKind::Settled { .. } => "Settled",
            EventKind::Claimed { .. } => "Claimed",
            EventKind::Refunded { .. } => "Refunded",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256};
    use serde_json::json;

    fn locked() -> ChainEvent {
        ChainEvent {
            chain_id: 84532,
            block_number: 18_200_311,
            tx_hash: b256!("893de03fa8ad3bce24c2abc167867168b647dc9b1192817b29d52aee452d79e5"),
            log_index: 4,
            kind: EventKind::Locked {
                lock_id: B256::repeat_byte(0x11),
                order_id: B256::repeat_byte(0x22),
                usdc_amount: Amount::from(12_036_000u64),
            },
        }
    }

    #[test]
    fn json_is_flat_with_type_and_camel_case_names() {
        let value = serde_json::to_value(locked()).unwrap();
        assert_eq!(
            value,
            json!({
                "chainId": 84532,
                "blockNumber": 18200311,
                "txHash": "0x893de03fa8ad3bce24c2abc167867168b647dc9b1192817b29d52aee452d79e5",
                "logIndex": 4,
                "type": "Locked",
                "lockId": format!("0x{}", "11".repeat(32)),
                "orderId": format!("0x{}", "22".repeat(32)),
                "usdcAmount": "12036000",
            })
        );
    }

    #[test]
    fn every_kind_round_trips_through_json() {
        let id = B256::repeat_byte(0x33);
        let amount = Amount::from(12_000_000u64);
        let kinds = [
            EventKind::Paid { payment_id: id, order_id: id, payer: address!("A917000000000000000000000000000000000001"), amount },
            EventKind::DirectRefunded { payment_id: id, order_id: id, amount, total_refunded: amount },
            EventKind::Locked { lock_id: id, order_id: id, usdc_amount: amount },
            EventKind::Filled { order_id: id, destination_lock_id: id, amount },
            EventKind::Settled { order_id: id, amount },
            EventKind::Claimed { lock_id: id },
            EventKind::Refunded { lock_id: id },
        ];
        for kind in kinds {
            let event = ChainEvent { kind, ..locked() };
            let text = serde_json::to_string(&event).unwrap();
            assert!(text.contains(&format!(r#""type":"{}""#, event.kind.name())), "{text}");
            assert_eq!(serde_json::from_str::<ChainEvent>(&text).unwrap(), event, "{text}");
        }
    }

    #[test]
    fn unknown_type_is_rejected() {
        let mut value = serde_json::to_value(locked()).unwrap();
        value["type"] = json!("Teleported");
        assert!(serde_json::from_value::<ChainEvent>(value).is_err());
    }
}
