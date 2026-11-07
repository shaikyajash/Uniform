//! The two locks of a cross-chain payment, as the contracts compute them. Both the Order Desk
//! (quotes, events) and the Relayer (checks before sending) use these, so they can never disagree.
//!
//! - SourceLock (customer chains): `lockIdFor(params, payer, token, amount)`, which is also the
//!   EIP-3009 nonce the customer signs.
//! - DestinationLock (Whitechain): `lockIdFor(srcChainId, srcLockId, maker)`.

use alloy::{
    primitives::{Address, B256, U256, keccak256},
    sol,
    sol_types::SolValue,
};
use serde::{Deserialize, Serialize};

use crate::amount::Amount;

/// `SourceLock.LOCK_TAG`.
const SOURCE_LOCK_TAG: &str = "uniform.sourcelock.lock.v2";
/// `DestinationLock.LOCK_TAG`.
const DESTINATION_LOCK_TAG: &str = "uniform.destinationlock.lock.v1";

sol! {
    /// `SourceLock.LockParams`, for ABI encoding.
    struct LockParams {
        bytes32 orderId;
        address receiver;
        uint256 minUsdcOut;
        bytes32 hashlock;
        uint64 expiry;
        address outToken;
    }
}

/// `SourceLock.LockParams`: what the customer's lock promises. Stored on the quote and in jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLockParams {
    pub order_id: B256,
    /// Who may claim it with the secret: Bob.
    pub receiver: Address,
    /// The USDC value the quote promised; SourceLock enforces it for USDC locks.
    pub min_usdc_out: Amount,
    pub hashlock: B256,
    /// Unix seconds; refundable to the customer from then on.
    pub expiry: u64,
    /// Zero (or USDC): Bob is paid in the token locked.
    pub out_token: Address,
}

impl SourceLockParams {
    pub fn to_sol(self) -> LockParams {
        LockParams {
            orderId: self.order_id,
            receiver: self.receiver,
            minUsdcOut: self.min_usdc_out.0,
            hashlock: self.hashlock,
            expiry: self.expiry,
            outToken: self.out_token,
        }
    }
}

/// `SourceLock.lockIdFor(params, payer, token, amount)` on `chain_id` at `source_lock`.
pub fn source_lock_id(
    chain_id: u64,
    source_lock: Address,
    params: &SourceLockParams,
    payer: Address,
    token: Address,
    amount: Amount,
) -> B256 {
    let encoded =
        (keccak256(SOURCE_LOCK_TAG), U256::from(chain_id), source_lock, params.to_sol(), payer, token, amount.0)
            .abi_encode();
    keccak256(encoded)
}

/// `DestinationLock.lockIdFor(srcChainId, srcLockId, maker)` on `chain_id` at `destination_lock`.
/// The one lock Bob (`maker`) can open for a given source lock.
pub fn destination_lock_id(
    chain_id: u64,
    destination_lock: Address,
    src_chain_id: u64,
    src_lock_id: B256,
    maker: Address,
) -> B256 {
    let encoded = (keccak256(DESTINATION_LOCK_TAG), U256::from(chain_id), destination_lock, U256::from(src_chain_id), src_lock_id, maker)
        .abi_encode();
    keccak256(encoded)
}

/// What the customer locks for an order: the amount plus Bob's fee, rounded up. The Matchmaker's
/// `required_usdc` uses the same formula, so a quote never falls one unit short of what Bob fills.
/// USDT and USDC both have 6 decimals.
pub fn with_provider_fee(order_amount: Amount, fee_bps: u16) -> Option<Amount> {
    let scaled = order_amount.0.checked_mul(U256::from(10_000u64 + u64::from(fee_bps)))?;
    Some(Amount(scaled.div_ceil(U256::from(10_000u64))))
}

/// The hashlock for a secret: `keccak256(abi.encodePacked(secret))`, as both locks check it.
pub fn hashlock_of(secret: B256) -> B256 {
    keccak256(secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256};

    const BASE_SOURCE_LOCK: Address = address!("02dF2659B5B4956EAFEe3293584C2aaefC114871");
    const BASE_USDC: Address = address!("036CbD53842c5426634e7929541eC2318f3dCF7e");
    const DESTINATION_LOCK: Address = address!("669c928135B848C828144360aD7a73391968BDA3");

    fn params() -> SourceLockParams {
        SourceLockParams {
            order_id: B256::repeat_byte(0xab),
            receiver: address!("b0b0000000000000000000000000000000000001"),
            min_usdc_out: Amount::from(12_036_000u64),
            hashlock: B256::repeat_byte(0x42),
            expiry: 1_800_001_200,
            out_token: Address::ZERO,
        }
    }

    /// Read from the deployed SourceLock on Base Sepolia (`lockIdFor` with these inputs, a free
    /// `eth_call`) on Fri Nov 6.
    #[test]
    fn source_lock_id_matches_the_deployed_contract() {
        let id = source_lock_id(
            84_532,
            BASE_SOURCE_LOCK,
            &params(),
            address!("a917000000000000000000000000000000000001"),
            BASE_USDC,
            Amount::from(12_036_000u64),
        );
        assert_eq!(id, b256!("69da5ed0b8db67dbecd73474dfa7309d9d05b13e5780348d99524a4c19a910e6"));
    }

    /// Read from the deployed DestinationLock on Whitechain (`lockIdFor`) on Fri Nov 6.
    #[test]
    fn destination_lock_id_matches_the_deployed_contract() {
        let id = destination_lock_id(
            1874,
            DESTINATION_LOCK,
            84_532,
            B256::repeat_byte(0xcd),
            address!("b0b0000000000000000000000000000000000001"),
        );
        assert_eq!(id, b256!("bb6ec8fb5efb33d4327fb269fc4a011965c466759ca012686871b7f31f2e2bee"));
    }

    #[test]
    fn the_fee_rounds_up_like_the_matchmaker() {
        let fee = |amount: u64| with_provider_fee(Amount::from(amount), 30).unwrap().to_string();
        assert_eq!(fee(12_000_000), "12036000");
        assert_eq!(fee(1), "2", "0.000001 USDT plus any fee is 0.000002 USDC");
        assert_eq!(fee(990_000), "992970", "M2's live lock: 0.99 USDT took 0.99297 USDC");
        assert_eq!(with_provider_fee(Amount::from(5u64), 0).unwrap().to_string(), "5");
    }

    #[test]
    fn hashlock_is_keccak_of_the_secret_bytes() {
        let secret = B256::repeat_byte(7);
        assert_eq!(hashlock_of(secret), keccak256(secret.as_slice()));
    }

    #[test]
    fn params_round_trip_as_camel_case_json() {
        let value = serde_json::to_value(params()).unwrap();
        assert_eq!(value["minUsdcOut"], "12036000");
        assert_eq!(value["expiry"], 1_800_001_200u64);
        assert_eq!(serde_json::from_value::<SourceLockParams>(value).unwrap(), params());
    }
}
