//! Turns contract logs into `common::events::ChainEvent`, the form the Order Desk accepts.

use alloy::{primitives::Log as PrimitiveLog, rpc::types::Log, sol_types::SolEvent};
use bindings::{DestinationLock, DirectPay, PayoutPool, SourceLock};
use common::{
    amount::Amount,
    events::{ChainEvent, EventKind},
};

/// Which of our contracts emitted a log. Logs are only read from our own addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    DirectPay,
    SourceLock,
    PayoutPool,
    DestinationLock,
}

impl Source {
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::DirectPay => "DirectPay",
            Source::SourceLock => "SourceLock",
            Source::PayoutPool => "PayoutPool",
            Source::DestinationLock => "DestinationLock",
        }
    }

    pub fn parse(text: &str) -> Option<Source> {
        [
            Source::DirectPay,
            Source::SourceLock,
            Source::PayoutPool,
            Source::DestinationLock,
        ]
        .into_iter()
        .find(|s| s.as_str() == text)
    }
}

/// A decoded log the Matchmaker acts on, besides reporting it.
#[derive(Debug, Clone)]
pub enum Action {
    Locked(SourceLock::Locked),
    Settled(DestinationLock::Settled),
    None,
}

/// The event to report (if the log is one the Order Desk tracks) and what to act on.
pub fn decode(chain_id: u64, source: Source, log: &Log) -> (Option<ChainEvent>, Action) {
    // A log removed by a reorg is no longer part of the chain.
    if log.removed {
        return (None, Action::None);
    }
    let (Some(block_number), Some(tx_hash), Some(log_index)) =
        (log.block_number, log.transaction_hash, log.log_index)
    else {
        return (None, Action::None);
    };
    let wrap = |kind| {
        Some(ChainEvent {
            chain_id,
            block_number,
            tx_hash,
            log_index,
            kind,
        })
    };
    let inner: &PrimitiveLog = &log.inner;
    let Some(topic0) = log.topic0().copied() else {
        return (None, Action::None);
    };

    match source {
        Source::DirectPay if topic0 == DirectPay::Paid::SIGNATURE_HASH => {
            match DirectPay::Paid::decode_log(inner) {
                Ok(e) => (
                    wrap(EventKind::Paid {
                        payment_id: e.paymentId,
                        order_id: e.orderId,
                        payer: e.payer,
                        amount: Amount::from(e.amount),
                    }),
                    Action::None,
                ),
                Err(_) => (None, Action::None),
            }
        }
        Source::DirectPay if topic0 == DirectPay::Refunded::SIGNATURE_HASH => {
            match DirectPay::Refunded::decode_log(inner) {
                Ok(e) => (
                    wrap(EventKind::DirectRefunded {
                        payment_id: e.paymentId,
                        order_id: e.orderId,
                        amount: Amount::from(e.amount),
                        total_refunded: Amount::from(e.totalRefunded),
                    }),
                    Action::None,
                ),
                Err(_) => (None, Action::None),
            }
        }
        Source::SourceLock if topic0 == SourceLock::Locked::SIGNATURE_HASH => {
            match SourceLock::Locked::decode_log(inner) {
                Ok(e) => (
                    wrap(EventKind::Locked {
                        lock_id: e.lockId,
                        order_id: e.orderId,
                        token: e.token,
                        amount: Amount::from(e.amount),
                    }),
                    Action::Locked(e.data),
                ),
                Err(_) => (None, Action::None),
            }
        }
        Source::SourceLock if topic0 == SourceLock::Claimed::SIGNATURE_HASH => {
            match SourceLock::Claimed::decode_log(inner) {
                Ok(e) => (wrap(EventKind::Claimed { lock_id: e.lockId }), Action::None),
                Err(_) => (None, Action::None),
            }
        }
        Source::SourceLock if topic0 == SourceLock::Refunded::SIGNATURE_HASH => {
            match SourceLock::Refunded::decode_log(inner) {
                Ok(e) => (
                    wrap(EventKind::Refunded { lock_id: e.lockId }),
                    Action::None,
                ),
                Err(_) => (None, Action::None),
            }
        }
        Source::PayoutPool if topic0 == PayoutPool::Filled::SIGNATURE_HASH => {
            match PayoutPool::Filled::decode_log(inner) {
                Ok(e) => (
                    wrap(EventKind::Filled {
                        order_id: e.orderId,
                        destination_lock_id: e.lockId,
                        amount: Amount::from(e.amount),
                    }),
                    Action::None,
                ),
                Err(_) => (None, Action::None),
            }
        }
        Source::DestinationLock if topic0 == DestinationLock::Settled::SIGNATURE_HASH => {
            match DestinationLock::Settled::decode_log(inner) {
                Ok(e) => (
                    wrap(EventKind::Settled {
                        order_id: e.orderId,
                        amount: Amount::from(e.amount),
                    }),
                    Action::Settled(e.data),
                ),
                Err(_) => (None, Action::None),
            }
        }
        Source::DestinationLock if topic0 == DestinationLock::Refunded::SIGNATURE_HASH => {
            match DestinationLock::Refunded::decode_log(inner) {
                Ok(e) => (
                    wrap(EventKind::Refunded { lock_id: e.lockId }),
                    Action::None,
                ),
                Err(_) => (None, Action::None),
            }
        }
        _ => (None, Action::None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, B256, Bytes, LogData, U256, address};

    fn rpc_log(address: Address, data: LogData) -> Log {
        Log {
            inner: PrimitiveLog { address, data },
            block_number: Some(100),
            transaction_hash: Some(B256::repeat_byte(0xaa)),
            log_index: Some(3),
            ..Default::default()
        }
    }

    const LOCK: Address = address!("02dF2659B5B4956EAFEe3293584C2aaefC114871");

    #[test]
    fn locked_is_reported_and_acted_on() {
        let event = SourceLock::Locked {
            lockId: B256::repeat_byte(1),
            orderId: B256::repeat_byte(2),
            payer: Address::repeat_byte(3),
            receiver: Address::repeat_byte(4),
            token: Address::repeat_byte(6),
            amount: U256::from(12_036_000u64),
            hashlock: B256::repeat_byte(5),
            expiry: 1_800_001_200,
        };
        let log = rpc_log(LOCK, event.encode_log_data());
        let (reported, action) = decode(84532, Source::SourceLock, &log);
        let reported = reported.unwrap();
        assert_eq!(reported.chain_id, 84532);
        assert_eq!(reported.block_number, 100);
        assert_eq!(reported.log_index, 3);
        assert_eq!(
            reported.kind,
            EventKind::Locked {
                lock_id: B256::repeat_byte(1),
                order_id: B256::repeat_byte(2),
                token: Address::repeat_byte(6),
                amount: Amount::from(U256::from(12_036_000u64)),
            }
        );
        assert!(matches!(action, Action::Locked(e) if e.receiver == Address::repeat_byte(4)));
    }

    #[test]
    fn settled_carries_the_secret_to_act_on() {
        let event = DestinationLock::Settled {
            lockId: B256::repeat_byte(1),
            orderId: B256::repeat_byte(2),
            merchant: Address::repeat_byte(3),
            amount: U256::from(12_000_000u64),
            srcChainId: U256::from(84532u64),
            srcLockId: B256::repeat_byte(4),
            secret: B256::repeat_byte(5),
        };
        let (reported, action) = decode(
            1874,
            Source::DestinationLock,
            &rpc_log(LOCK, event.encode_log_data()),
        );
        assert_eq!(
            reported.unwrap().kind,
            EventKind::Settled {
                order_id: B256::repeat_byte(2),
                amount: Amount::from(U256::from(12_000_000u64))
            }
        );
        assert!(matches!(action, Action::Settled(e) if e.secret == B256::repeat_byte(5)));
    }

    #[test]
    fn a_log_from_the_wrong_kind_of_contract_is_ignored() {
        let event = SourceLock::Claimed {
            lockId: B256::repeat_byte(1),
            secret: B256::repeat_byte(2),
        };
        let log = rpc_log(LOCK, event.encode_log_data());
        let (reported, action) = decode(1874, Source::DestinationLock, &log);
        assert!(reported.is_none());
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn unknown_or_pending_logs_are_ignored() {
        let unknown = rpc_log(
            LOCK,
            LogData::new_unchecked(vec![B256::repeat_byte(9)], Bytes::new()),
        );
        assert!(decode(84532, Source::SourceLock, &unknown).0.is_none());

        let mut pending = rpc_log(
            LOCK,
            SourceLock::Refunded {
                lockId: B256::repeat_byte(1),
            }
            .encode_log_data(),
        );
        pending.block_number = None;
        assert!(decode(84532, Source::SourceLock, &pending).0.is_none());
    }

    #[test]
    fn a_log_removed_by_a_reorg_is_ignored() {
        let mut log = rpc_log(
            LOCK,
            SourceLock::Refunded {
                lockId: B256::repeat_byte(1),
            }
            .encode_log_data(),
        );
        log.removed = true;
        assert!(decode(84532, Source::SourceLock, &log).0.is_none());
    }

    #[test]
    fn the_other_kinds_map_to_the_order_desk_names() {
        let id = B256::repeat_byte(1);
        let amount = U256::from(5u64);
        let cases: Vec<(Source, LogData, &str)> = vec![
            (
                Source::DirectPay,
                DirectPay::Paid {
                    paymentId: id,
                    orderId: id,
                    merchant: Address::ZERO,
                    payer: Address::ZERO,
                    amount,
                }
                .encode_log_data(),
                "Paid",
            ),
            (
                Source::DirectPay,
                DirectPay::Refunded {
                    paymentId: id,
                    orderId: id,
                    payer: Address::ZERO,
                    amount,
                    totalRefunded: amount,
                }
                .encode_log_data(),
                "DirectRefunded",
            ),
            (
                Source::PayoutPool,
                PayoutPool::Filled {
                    orderId: id,
                    maker: Address::ZERO,
                    strategyHash: id,
                    amount,
                    lockId: id,
                }
                .encode_log_data(),
                "Filled",
            ),
            (
                Source::SourceLock,
                SourceLock::Claimed {
                    lockId: id,
                    secret: id,
                }
                .encode_log_data(),
                "Claimed",
            ),
            (
                Source::SourceLock,
                SourceLock::Refunded { lockId: id }.encode_log_data(),
                "Refunded",
            ),
            (
                Source::DestinationLock,
                DestinationLock::Refunded {
                    lockId: id,
                    maker: Address::ZERO,
                    amount,
                    backToAqua: true,
                }
                .encode_log_data(),
                "Refunded",
            ),
        ];
        for (source, data, name) in cases {
            let event = decode(1, source, &rpc_log(LOCK, data))
                .0
                .unwrap_or_else(|| panic!("{name} not decoded"));
            assert_eq!(event.kind.name(), name);
        }
    }
}
