//! Pure decisions: whether to fill a customer's source lock, and with what expiry. No chain or
//! network access here, so every rule is unit-tested.

use alloy::primitives::{Address, B256, U256};
use common::status::OrderStatus;

/// Timing rules, from `timeoutsSeconds` in chains.json and the contracts' limits.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Fill only if the source lock has at least this long left.
    pub fill_cutoff: u64,
    /// How long the destination lock stays open.
    pub destination_lock: u64,
    /// The destination lock must end at least this long before the source lock.
    pub min_gap: u64,
    /// DestinationLock rejects locks shorter than this.
    pub min_destination_lock: u64,
}

/// Bob's strategy limits, as shipped on Aqua.
#[derive(Debug, Clone, Copy)]
pub struct Provider {
    pub address: Address,
    pub max_per_order: U256,
    pub fee_bps: u16,
}

/// A source lock as stored on chain (`SourceLock.locks(lockId)`).
#[derive(Debug, Clone, Copy)]
pub struct SourceLockView {
    pub open: bool,
    pub receiver: Address,
    pub amount: U256,
    pub hashlock: B256,
    pub expiry: u64,
}

/// What the Order Desk says about the order.
#[derive(Debug, Clone, Copy)]
pub struct OrderView {
    pub merchant: Address,
    /// In USDT's smallest units (6 decimals).
    pub amount: U256,
    pub status: OrderStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FillPlan {
    pub merchant: Address,
    pub amount: U256,
    pub destination_expiry: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    NotOpen,
    NotForProvider(Address),
    ZeroHashlock,
    OrderNotPayable(OrderStatus),
    ZeroOrderAmount,
    NotEnoughLocked { locked: U256, needed: U256 },
    AboveMaxPerOrder { amount: U256, max: U256 },
    TooLittleTime { left: u64, needed: u64 },
    AlreadyFilled,
}

/// The USDC the customer must lock for an order: the amount plus Bob's fee, rounded up.
/// USDC and USDT both have 6 decimals. `None` if the amount is too large to compute.
pub fn required_usdc(order_amount: U256, fee_bps: u16) -> Option<U256> {
    let scaled = order_amount.checked_mul(U256::from(10_000u64 + u64::from(fee_bps)))?;
    Some(scaled.div_ceil(U256::from(10_000u64)))
}

/// The destination lock's expiry, on the destination chain's clock: `destination_lock` from now,
/// and ending at least `min_gap` before the source lock. The source lock's time left is measured
/// on its own chain and carried over, so a clock difference between the chains cannot shrink the
/// gap. `None` if that leaves less than the contract's minimum.
pub fn destination_expiry(dest_now: u64, source_left: u64, timing: Timing) -> Option<u64> {
    let wanted = dest_now.checked_add(timing.destination_lock)?;
    let latest = dest_now.checked_add(source_left.checked_sub(timing.min_gap)?)?;
    let expiry = wanted.min(latest);
    (expiry >= dest_now.checked_add(timing.min_destination_lock)?).then_some(expiry)
}

/// Whether to fill this source lock, and how. `src_now` is the source chain's time and
/// `dest_now` the destination chain's.
pub fn fill_decision(
    lock: &SourceLockView,
    order: &OrderView,
    provider: &Provider,
    already_filled: bool,
    src_now: u64,
    dest_now: u64,
    timing: Timing,
) -> Result<FillPlan, Skip> {
    if already_filled {
        return Err(Skip::AlreadyFilled);
    }
    if !lock.open {
        return Err(Skip::NotOpen);
    }
    if lock.receiver != provider.address {
        return Err(Skip::NotForProvider(lock.receiver));
    }
    if lock.hashlock == B256::ZERO {
        return Err(Skip::ZeroHashlock);
    }
    if !matches!(
        order.status,
        OrderStatus::Quoted | OrderStatus::Paying | OrderStatus::SourceLocked
    ) {
        return Err(Skip::OrderNotPayable(order.status));
    }
    if order.amount.is_zero() {
        return Err(Skip::ZeroOrderAmount);
    }
    if order.amount > provider.max_per_order {
        return Err(Skip::AboveMaxPerOrder {
            amount: order.amount,
            max: provider.max_per_order,
        });
    }
    let needed = required_usdc(order.amount, provider.fee_bps).ok_or(Skip::AboveMaxPerOrder {
        amount: order.amount,
        max: provider.max_per_order,
    })?;
    if lock.amount < needed {
        return Err(Skip::NotEnoughLocked {
            locked: lock.amount,
            needed,
        });
    }
    let left = lock.expiry.saturating_sub(src_now);
    if left < timing.fill_cutoff {
        return Err(Skip::TooLittleTime {
            left,
            needed: timing.fill_cutoff,
        });
    }
    let destination_expiry =
        destination_expiry(dest_now, left, timing).ok_or(Skip::TooLittleTime {
            left,
            needed: timing.min_gap + timing.min_destination_lock,
        })?;
    Ok(FillPlan {
        merchant: order.merchant,
        amount: order.amount,
        destination_expiry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const NOW: u64 = 1_800_000_000;
    const BOB: Address = address!("C2bF8030f00CAB26c4611f93c5F71b9d01Ec120A");
    const SHOP: Address = address!("effA754149A8646De8D9CDE42f9f61a9A4D554Fe");

    fn timing() -> Timing {
        Timing {
            fill_cutoff: 900,
            destination_lock: 600,
            min_gap: 300,
            min_destination_lock: 300,
        }
    }

    fn provider() -> Provider {
        Provider {
            address: BOB,
            max_per_order: U256::from(20_000_000u64),
            fee_bps: 30,
        }
    }

    fn lock() -> SourceLockView {
        SourceLockView {
            open: true,
            receiver: BOB,
            amount: U256::from(12_036_000u64),
            hashlock: B256::repeat_byte(0xab),
            expiry: NOW + 1200,
        }
    }

    fn order() -> OrderView {
        OrderView {
            merchant: SHOP,
            amount: U256::from(12_000_000u64),
            status: OrderStatus::Quoted,
        }
    }

    fn decide(lock: SourceLockView, order: OrderView) -> Result<FillPlan, Skip> {
        fill_decision(&lock, &order, &provider(), false, NOW, NOW, timing())
    }

    #[test]
    fn fee_is_added_and_rounded_up() {
        assert_eq!(
            required_usdc(U256::from(12_000_000u64), 30),
            Some(U256::from(12_036_000u64))
        );
        assert_eq!(required_usdc(U256::from(1u64), 30), Some(U256::from(2u64)));
        assert_eq!(
            required_usdc(U256::from(1_000_000u64), 0),
            Some(U256::from(1_000_000u64))
        );
        assert_eq!(required_usdc(U256::MAX, 30), None);
    }

    #[test]
    fn a_good_lock_is_filled_for_the_order_amount_with_a_ten_minute_lock() {
        let plan = decide(lock(), order()).unwrap();
        assert_eq!(
            plan,
            FillPlan {
                merchant: SHOP,
                amount: U256::from(12_000_000u64),
                destination_expiry: NOW + 600
            }
        );
    }

    #[test]
    fn destination_ends_at_least_the_gap_before_the_source() {
        let mut l = lock();
        l.expiry = NOW + 900;
        let plan = decide(l, order()).unwrap();
        assert_eq!(plan.destination_expiry, NOW + 600);
        assert!(l.expiry - plan.destination_expiry >= timing().min_gap);
    }

    #[test]
    fn each_skip_reason() {
        assert_eq!(
            fill_decision(&lock(), &order(), &provider(), true, NOW, NOW, timing()),
            Err(Skip::AlreadyFilled)
        );

        let mut l = lock();
        l.open = false;
        assert_eq!(decide(l, order()), Err(Skip::NotOpen));

        let mut l = lock();
        l.receiver = SHOP;
        assert_eq!(decide(l, order()), Err(Skip::NotForProvider(SHOP)));

        let mut l = lock();
        l.hashlock = B256::ZERO;
        assert_eq!(decide(l, order()), Err(Skip::ZeroHashlock));

        for status in [
            OrderStatus::Created,
            OrderStatus::Paid,
            OrderStatus::DestinationLocked,
            OrderStatus::Expired,
        ] {
            let mut o = order();
            o.status = status;
            assert_eq!(decide(lock(), o), Err(Skip::OrderNotPayable(status)));
        }

        let mut o = order();
        o.amount = U256::ZERO;
        assert_eq!(decide(lock(), o), Err(Skip::ZeroOrderAmount));

        let mut l = lock();
        l.amount = U256::from(12_035_999u64);
        assert_eq!(
            decide(l, order()),
            Err(Skip::NotEnoughLocked {
                locked: U256::from(12_035_999u64),
                needed: U256::from(12_036_000u64)
            })
        );

        let mut o = order();
        o.amount = U256::from(21_000_000u64);
        let mut l = lock();
        l.amount = U256::from(30_000_000u64);
        assert_eq!(
            decide(l, o),
            Err(Skip::AboveMaxPerOrder {
                amount: U256::from(21_000_000u64),
                max: U256::from(20_000_000u64)
            })
        );

        let mut l = lock();
        l.expiry = NOW + 899;
        assert_eq!(
            decide(l, order()),
            Err(Skip::TooLittleTime {
                left: 899,
                needed: 900
            })
        );
    }

    #[test]
    fn an_expired_lock_is_too_little_time_not_a_panic() {
        let mut l = lock();
        l.expiry = NOW - 10;
        assert_eq!(
            decide(l, order()),
            Err(Skip::TooLittleTime {
                left: 0,
                needed: 900
            })
        );
    }

    #[test]
    fn source_locked_and_paying_orders_are_still_payable() {
        for status in [OrderStatus::Paying, OrderStatus::SourceLocked] {
            let mut o = order();
            o.status = status;
            assert!(decide(lock(), o).is_ok());
        }
    }

    #[test]
    fn destination_expiry_refuses_when_the_gap_leaves_too_little() {
        let t = Timing {
            fill_cutoff: 0,
            ..timing()
        };
        assert_eq!(destination_expiry(NOW, 599, t), None);
        assert_eq!(destination_expiry(NOW, 600, t), Some(NOW + 300));
        assert_eq!(destination_expiry(NOW, 100, t), None);
        assert_eq!(destination_expiry(NOW, 0, t), None);
    }

    #[test]
    fn a_slow_destination_clock_cannot_shrink_the_gap() {
        // Whitechain's clock is 200 s behind Base: the gap is measured in time left, not timestamps.
        let mut l = lock();
        l.expiry = NOW + 900;
        let plan =
            fill_decision(&l, &order(), &provider(), false, NOW, NOW - 200, timing()).unwrap();
        assert_eq!(plan.destination_expiry, NOW - 200 + 600);
        let source_left = l.expiry - NOW;
        let dest_left = plan.destination_expiry - (NOW - 200);
        assert!(source_left - dest_left >= timing().min_gap);
    }

    #[test]
    fn a_huge_order_is_capped_before_the_fee_math() {
        let mut o = order();
        o.amount = U256::MAX;
        let mut l = lock();
        l.amount = U256::MAX;
        assert_eq!(
            decide(l, o),
            Err(Skip::AboveMaxPerOrder {
                amount: U256::MAX,
                max: U256::from(20_000_000u64)
            })
        );
    }
}
