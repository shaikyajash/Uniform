//! Token amounts in the smallest unit, written as decimal text.

use std::{fmt, str::FromStr};

use alloy::primitives::U256;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

/// A token amount in its smallest unit: `12036000` is 12.036 USDC (6 decimals).
///
/// JSON and the database hold it as decimal text, `"12036000"`: a JSON number loses digits
/// above 2^53, and alloy's own `U256` format is hex.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Amount(pub U256);

impl From<U256> for Amount {
    fn from(value: U256) -> Self {
        Amount(value)
    }
}

impl From<u64> for Amount {
    fn from(value: u64) -> Self {
        Amount(U256::from(value))
    }
}

impl From<Amount> for U256 {
    fn from(amount: Amount) -> Self {
        amount.0
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// Text that is not a whole number of smallest units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidAmount(pub String);

impl fmt::Display for InvalidAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid amount {:?}: expected decimal digits of the smallest unit", self.0)
    }
}

impl std::error::Error for InvalidAmount {}

impl FromStr for Amount {
    type Err = InvalidAmount;

    /// Decimal digits only. `U256` alone reads `""` as zero and skips `_`, so check first.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || InvalidAmount(text.to_owned());
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        U256::from_str_radix(text, 10).map(Amount).map_err(|_| invalid())
    }
}

impl Serialize for Amount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Amount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_decimal_text() {
        let amount = Amount::from(12_036_000u64);
        assert_eq!(serde_json::to_string(&amount).unwrap(), r#""12036000""#);
        assert_eq!(serde_json::from_str::<Amount>(r#""12036000""#).unwrap(), amount);
    }

    #[test]
    fn amounts_above_two_to_the_53_keep_every_digit() {
        let big = Amount(U256::from(u128::MAX));
        let text = serde_json::to_string(&big).unwrap();
        assert_eq!(text, r#""340282366920938463463374607431768211455""#);
        assert_eq!(serde_json::from_str::<Amount>(&text).unwrap(), big);
    }

    #[test]
    fn hex_numbers_and_signs_are_rejected() {
        for bad in [r#""0xb7a720""#, r#""-5""#, r#""12.5""#, r#""""#, r#""1_000""#, r#"" 5""#, "12036000"] {
            assert!(serde_json::from_str::<Amount>(bad).is_err(), "{bad} should be rejected");
        }
        let too_big = format!("1{}", "0".repeat(80));
        assert_eq!(too_big.parse::<Amount>(), Err(InvalidAmount(too_big.clone())));
    }
}
