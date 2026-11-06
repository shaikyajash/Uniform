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

/// A source lock as stored on chain (`SourceLock.locks(lockId)`), with its token valued in USDC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLockView {
    pub open: bool,
    pub receiver: Address,
    /// What Bob credits the lock, in USDC's smallest units (see `usdc_value`).
    pub usdc_value: U256,
    pub hashlock: B256,
    pub expiry: u64,
}

/// The token a source lock holds. Only these two can be locked (SourceLock's allowed tokens).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockedToken {
    Usdc,
    Eurc,
}

/// What a lock is worth to Bob in USDC: USDC at face value; EURC at `eurc_micro_usd` (one EURC in
/// micro-dollars) less `buffer_bps`, rounded down. The buffer covers the price moving before Bob
/// converts the EURC. All three tokens have 6 decimals. `None` if the value cannot be computed.
pub fn usdc_value(
    token: LockedToken,
    amount: U256,
    eurc_micro_usd: u64,
    buffer_bps: u16,
) -> Option<U256> {
    match token {
        LockedToken::Usdc => Some(amount),
        LockedToken::Eurc => {
            let keep = 10_000u64.checked_sub(u64::from(buffer_bps))?;
            let scaled = amount
                .checked_mul(U256::from(eurc_micro_usd))?
                .checked_mul(U256::from(keep))?;
            Some(scaled / U256::from(10_000_000_000u64))
        }
    }
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
    NotEnoughLocked {
        locked: U256,
        needed: U256,
    },
    AboveMaxPerOrder {
        amount: U256,
        max: U256,
    },
    TooLittleTime {
        left: u64,
        needed: u64,
    },
    AlreadyFilled,
    /// A split order's locks are not all under one hashlock, so one secret cannot redeem them all.
    HashlockMismatch,
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

/// A split order's source locks as one lock for `fill_decision`: open only if every lock is open,
/// for one receiver and under one hashlock; worth the sum of their values. Each lock comes with its
/// own chain's time, so `expiry` is the shortest time left of any lock: pass `src_now = 0` with
/// the result.
pub fn combine(locks: &[(SourceLockView, u64)]) -> Result<SourceLockView, Skip> {
    let (first, _) = locks.first().ok_or(Skip::NotOpen)?;
    let mut combined = SourceLockView {
        usdc_value: U256::ZERO,
        expiry: u64::MAX,
        ..*first
    };
    for (lock, src_now) in locks {
        if !lock.open {
            return Err(Skip::NotOpen);
        }
        if lock.receiver != first.receiver {
            return Err(Skip::NotForProvider(lock.receiver));
        }
        if lock.hashlock != first.hashlock {
            return Err(Skip::HashlockMismatch);
        }
        combined.usdc_value = combined.usdc_value.saturating_add(lock.usdc_value);
        combined.expiry = combined.expiry.min(lock.expiry.saturating_sub(*src_now));
    }
    Ok(combined)
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
    if lock.usdc_value < needed {
        return Err(Skip::NotEnoughLocked {
            locked: lock.usdc_value,
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
            needed: timing.min_gap.saturating_add(timing.min_destination_lock),
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
            usdc_value: U256::from(12_036_000u64),
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
    fn usdc_is_valued_at_face_value() {
        let amount = U256::from(12_036_000u64);
        assert_eq!(
            usdc_value(LockedToken::Usdc, amount, 1_120_000, 100),
            Some(amount)
        );
    }

    #[test]
    fn eurc_is_valued_at_the_price_less_the_buffer_rounded_down() {
        // 10 EURC at 1.12 USD, less 1%: 11.088 USDC.
        assert_eq!(
            usdc_value(LockedToken::Eurc, U256::from(10_000_000u64), 1_120_000, 100),
            Some(U256::from(11_088_000u64))
        );
        assert_eq!(
            usdc_value(LockedToken::Eurc, U256::from(1u64), 1_120_000, 100),
            Some(U256::from(1u64))
        );
        assert_eq!(
            usdc_value(LockedToken::Eurc, U256::from(10_000_000u64), 1_120_000, 0),
            Some(U256::from(11_200_000u64))
        );
    }

    #[test]
    fn an_eurc_lock_must_cover_the_order_after_the_buffer() {
        // 12 USDT + 0.3% = 12.036 USDC. At 1.12 less 1%, that needs 10.854 EURC (rounded up).
        let needed = required_usdc(U256::from(12_000_000u64), 30).unwrap();
        let enough = usdc_value(LockedToken::Eurc, U256::from(10_855_000u64), 1_120_000, 100);
        let short = usdc_value(LockedToken::Eurc, U256::from(10_854_000u64), 1_120_000, 100);
        assert!(enough.unwrap() >= needed);
        assert!(short.unwrap() < needed);

        let mut l = lock();
        l.usdc_value = short.unwrap();
        assert!(matches!(
            decide(l, order()),
            Err(Skip::NotEnoughLocked { .. })
        ));
        l.usdc_value = enough.unwrap();
        assert!(decide(l, order()).is_ok());
    }

    #[test]
    fn the_spec_quote_formula_always_covers_the_order() {
        // docs/SPEC.md: EURC to ask = ceil(required × 10^10 / (price × (10000 − buffer))).
        for (price, buffer) in [(1_120_000u64, 100u16), (1_083_457, 0), (999_999, 250)] {
            for order in [1u64, 999_999, 12_000_000, 19_999_999] {
                let needed = required_usdc(U256::from(order), 30).unwrap();
                let divisor = U256::from(price * (10_000 - u64::from(buffer)));
                let ask = (needed * U256::from(10_000_000_000u64)).div_ceil(divisor);
                let value = usdc_value(LockedToken::Eurc, ask, price, buffer).unwrap();
                assert!(value >= needed, "price {price} buffer {buffer} order {order}");
                let less = usdc_value(LockedToken::Eurc, ask - U256::from(1u64), price, buffer);
                assert!(less.unwrap() < needed, "ask is not the minimum");
            }
        }
    }

    #[test]
    fn eurc_value_math_cannot_overflow_or_take_a_full_buffer() {
        assert_eq!(
            usdc_value(LockedToken::Eurc, U256::MAX, 1_120_000, 100),
            None
        );
        assert_eq!(
            usdc_value(LockedToken::Eurc, U256::from(1u64), 1_120_000, 10_001),
            None
        );
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
        l.usdc_value = U256::from(12_035_999u64);
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
        l.usdc_value = U256::from(30_000_000u64);
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
        l.usdc_value = U256::MAX;
        assert_eq!(
            decide(l, o),
            Err(Skip::AboveMaxPerOrder {
                amount: U256::MAX,
                max: U256::from(20_000_000u64)
            })
        );
    }

    #[test]
    fn split_locks_add_up_and_take_the_shortest_time_left() {
        // 6 USDC on one chain and 6.036 on another, whose clock is 100 s ahead.
        let mut a = lock();
        a.usdc_value = U256::from(6_000_000u64);
        let mut b = lock();
        b.usdc_value = U256::from(6_036_000u64);
        b.expiry = NOW + 100 + 1000;
        let combined = combine(&[(a, NOW), (b, NOW + 100)]).unwrap();
        assert_eq!(combined.usdc_value, U256::from(12_036_000u64));
        assert_eq!(combined.expiry, 1000);
        let plan =
            fill_decision(&combined, &order(), &provider(), false, 0, NOW, timing()).unwrap();
        assert_eq!(plan.amount, U256::from(12_000_000u64));
        assert_eq!(plan.destination_expiry, NOW + 600);
    }

    #[test]
    fn split_locks_must_each_be_open_for_bob_under_one_hashlock() {
        let mut half = lock();
        half.usdc_value = U256::from(6_018_000u64);
        let mut closed = half;
        closed.open = false;
        assert_eq!(combine(&[(half, NOW), (closed, NOW)]), Err(Skip::NotOpen));
        let mut stranger = half;
        stranger.receiver = SHOP;
        assert_eq!(
            combine(&[(half, NOW), (stranger, NOW)]),
            Err(Skip::NotForProvider(SHOP))
        );
        let mut other = half;
        other.hashlock = B256::repeat_byte(0xcd);
        assert_eq!(
            combine(&[(half, NOW), (other, NOW)]),
            Err(Skip::HashlockMismatch)
        );
        assert_eq!(combine(&[]), Err(Skip::NotOpen));
    }

    #[test]
    fn split_locks_that_fall_short_together_are_not_filled() {
        let mut a = lock();
        a.usdc_value = U256::from(6_000_000u64);
        let mut b = lock();
        b.usdc_value = U256::from(6_000_000u64);
        let combined = combine(&[(a, NOW), (b, NOW)]).unwrap();
        assert_eq!(
            fill_decision(&combined, &order(), &provider(), false, 0, NOW, timing()),
            Err(Skip::NotEnoughLocked {
                locked: U256::from(12_000_000u64),
                needed: U256::from(12_036_000u64)
            })
        );
    }
}
