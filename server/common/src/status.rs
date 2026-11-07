//! Order, leg and refund statuses, and which order status moves are allowed.
//!
//! Mirrors `shared/types.ts` (`ORDER_STATUSES`, `ORDER_TRANSITIONS`). Change both together.

use std::{error::Error, fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// A text the database or the API sent that matches no variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownValue(pub String);

impl fmt::Display for UnknownValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown value: {:?}", self.0)
    }
}

impl Error for UnknownValue {}

/// Declares an enum stored as text: the same string in JSON, in the database and in `shared/types.ts`.
macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = UnknownValue;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|value| value.as_str() == text)
                    .ok_or_else(|| UnknownValue(text.to_owned()))
            }
        }
    };
}

text_enum! {
    /// Where an order is. Stored in `orders.status`.
    OrderStatus {
        /// The shop made the order; nobody has paid.
        Created => "created",
        /// The customer got a price and the data to sign.
        Quoted => "quoted",
        /// Signature received, transaction sent, waiting for the chain event.
        Paying => "paying",
        /// Cross-chain: every customer leg is locked (`Locked` events).
        SourceLocked => "source_locked",
        /// Cross-chain: Bob's USDT is locked for the shop (`Filled`).
        DestinationLocked => "destination_locked",
        /// The shop has the USDT (`Paid` or `Settled`).
        Paid => "paid",
        /// The shop gave back part of the amount.
        PartiallyRefunded => "partially_refunded",
        /// The shop gave back the full amount.
        Refunded => "refunded",
        /// The quote ran out before any money moved.
        Expired => "expired",
        /// Cross-chain timed out and the customer's locked money went back.
        Returned => "returned",
    }
}

impl OrderStatus {
    /// The statuses this one may move to. Same table as `ORDER_TRANSITIONS`.
    pub const fn next(self) -> &'static [OrderStatus] {
        use OrderStatus::*;
        match self {
            Created => &[Quoted, Expired],
            // A new quote can replace an old one.
            Quoted => &[Quoted, Paying, Expired],
            // Back to quoted if the transaction fails.
            Paying => &[Paid, SourceLocked, Quoted],
            SourceLocked => &[DestinationLocked, Returned],
            DestinationLocked => &[Paid, Returned],
            Paid => &[PartiallyRefunded, Refunded],
            PartiallyRefunded => &[PartiallyRefunded, Refunded],
            Refunded | Expired | Returned => &[],
        }
    }

    pub fn can_move(self, to: OrderStatus) -> bool {
        self.next().contains(&to)
    }
}

text_enum! {
    /// Where one cross-chain leg is. Stored in `legs.status`.
    LegStatus {
        Pending => "pending",
        Locked => "locked",
        Claimed => "claimed",
        Returned => "returned",
    }
}

text_enum! {
    /// Where one refund is. Stored in `refunds.status`.
    RefundStatus {
        Pending => "pending",
        Sent => "sent",
        Failed => "failed",
    }
}

text_enum! {
    /// How the customer pays.
    PayPath {
        /// Path A: USDT on Whitechain through DirectPay.
        Direct => "direct",
        /// Path B: one source chain, Bob pays the shop.
        CrossChain => "crosschain",
        /// Path C: legs on two source chains, all or nothing.
        Split => "split",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use OrderStatus::*;

    /// Every allowed move, written out from `ORDER_TRANSITIONS` in `shared/types.ts`.
    const ALLOWED: &[(OrderStatus, OrderStatus)] = &[
        (Created, Quoted),
        (Created, Expired),
        (Quoted, Quoted),
        (Quoted, Paying),
        (Quoted, Expired),
        (Paying, Paid),
        (Paying, SourceLocked),
        (Paying, Quoted),
        (SourceLocked, DestinationLocked),
        (SourceLocked, Returned),
        (DestinationLocked, Paid),
        (DestinationLocked, Returned),
        (Paid, PartiallyRefunded),
        (Paid, Refunded),
        (PartiallyRefunded, PartiallyRefunded),
        (PartiallyRefunded, Refunded),
    ];

    #[test]
    fn can_move_matches_the_shared_table_for_every_pair() {
        for &from in OrderStatus::ALL {
            for &to in OrderStatus::ALL {
                let expected = ALLOWED.contains(&(from, to));
                assert_eq!(from.can_move(to), expected, "{from} -> {to}");
            }
        }
    }

    #[test]
    fn final_statuses_have_no_moves() {
        for status in [Refunded, Expired, Returned] {
            assert!(status.next().is_empty(), "{status} should be final");
        }
    }

    #[test]
    fn text_round_trips_for_every_variant() {
        for &status in OrderStatus::ALL {
            assert_eq!(status.as_str().parse::<OrderStatus>(), Ok(status));
        }
        for &status in LegStatus::ALL {
            assert_eq!(status.as_str().parse::<LegStatus>(), Ok(status));
        }
        for &status in RefundStatus::ALL {
            assert_eq!(status.as_str().parse::<RefundStatus>(), Ok(status));
        }
        for &path in PayPath::ALL {
            assert_eq!(path.as_str().parse::<PayPath>(), Ok(path));
        }
    }

    #[test]
    fn json_uses_the_shared_names() {
        assert_eq!(serde_json::to_string(&SourceLocked).unwrap(), r#""source_locked""#);
        assert_eq!(serde_json::to_string(&PayPath::CrossChain).unwrap(), r#""crosschain""#);
        assert_eq!(serde_json::from_str::<OrderStatus>(r#""destination_locked""#).unwrap(), DestinationLocked);
    }

    #[test]
    fn old_names_are_rejected() {
        assert_eq!("shop_locked".parse::<OrderStatus>(), Err(UnknownValue("shop_locked".to_owned())));
        assert!("customer_locked".parse::<OrderStatus>().is_err());
    }
}
