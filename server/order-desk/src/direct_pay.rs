//! What a Path A customer signs: an EIP-3009 `ReceiveWithAuthorization` note that lets DirectPay
//! (deployed, A2) pull USDT from the customer and pay the shop. Signing spec: docs/SPEC.md.

use alloy::primitives::{Address, B256, U256, keccak256};
use alloy::sol_types::SolValue;
use common::{amount::Amount, chains::Token};
use serde_json::{Value, json};

/// `DirectPay.PAY_TAG`.
const PAY_TAG: &str = "uniform.directpay.pay.v1";
/// `DirectPay.REFUND_TAG`.
const REFUND_TAG: &str = "uniform.directpay.refund.v1";

/// Same as the contract's `paymentIdFor(orderId, merchant, amount, payer)`. It is also the note's
/// nonce, so a signature works only for this order, shop, amount and payer, and only once.
pub fn payment_id_for(
    chain_id: u64,
    direct_pay: Address,
    order_id: B256,
    merchant: Address,
    amount: Amount,
    payer: Address,
) -> B256 {
    let encoded = (keccak256(PAY_TAG), U256::from(chain_id), direct_pay, order_id, merchant, amount.0, payer)
        .abi_encode();
    keccak256(encoded)
}

/// Same as the contract's `refundNonceFor(paymentId, amount, refundedBefore)`: the nonce of the
/// shop's refund note. It commits to what was refunded before, so each note works for one refund.
pub fn refund_nonce_for(
    chain_id: u64,
    direct_pay: Address,
    payment_id: B256,
    amount: Amount,
    refunded_before: Amount,
) -> B256 {
    let encoded =
        (keccak256(REFUND_TAG), U256::from(chain_id), direct_pay, payment_id, amount.0, refunded_before.0).abi_encode();
    keccak256(encoded)
}

/// The note's fields. `to` is DirectPay: `receiveWithAuthorization` only accepts a call from `to`.
pub struct Note {
    pub from: Address,
    pub to: Address,
    pub value: Amount,
    pub valid_after: i64,
    pub valid_before: i64,
    pub nonce: B256,
}

/// The EIP-712 JSON the pay page passes to the wallet. Numbers are decimal text and addresses
/// lowercase hex, the JSON form wallets accept (checked with Privy in B0).
pub fn typed_data(token: &Token, chain_id: u64, note: &Note) -> Value {
    // Only tokens with EIP-3009 notes come here; chains.json gives each of them its domain.
    let (name, version) = token.eip712.as_ref().map_or(("", ""), |d| (d.name.as_str(), d.version.as_str()));
    json!({
        "domain": {
            "name": name,
            "version": version,
            "chainId": chain_id,
            "verifyingContract": format!("{:#x}", token.address),
        },
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "version", "type": "string" },
                { "name": "chainId", "type": "uint256" },
                { "name": "verifyingContract", "type": "address" },
            ],
            "ReceiveWithAuthorization": [
                { "name": "from", "type": "address" },
                { "name": "to", "type": "address" },
                { "name": "value", "type": "uint256" },
                { "name": "validAfter", "type": "uint256" },
                { "name": "validBefore", "type": "uint256" },
                { "name": "nonce", "type": "bytes32" },
            ],
        },
        "primaryType": "ReceiveWithAuthorization",
        "message": {
            "from": format!("{:#x}", note.from),
            "to": format!("{:#x}", note.to),
            "value": note.value.to_string(),
            "validAfter": note.valid_after.to_string(),
            "validBefore": note.valid_before.to_string(),
            "nonce": note.nonce.to_string(),
        },
    })
}

/// The same note as a `TransferWithAuthorization`: anyone may submit it, and the USDT goes straight
/// to `to`. Used for a shop's refund to the customer.
pub fn transfer_typed_data(token: &Token, chain_id: u64, note: &Note) -> Value {
    let mut data = typed_data(token, chain_id, note);
    let fields = data["types"]["ReceiveWithAuthorization"].take();
    let types = data["types"].as_object_mut().expect("types is an object");
    types.remove("ReceiveWithAuthorization");
    types.insert("TransferWithAuthorization".into(), fields);
    data["primaryType"] = json!("TransferWithAuthorization");
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        dyn_abi::TypedData,
        primitives::{address, b256},
        sol,
        sol_types::{SolStruct, eip712_domain},
    };
    use common::chains::Eip712Domain;

    const DIRECT_PAY: Address = address!("9DF1C7C32Ce3f1322e1c7bAEc30664b1c472DaE9");
    const USDT: Address = address!("cedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7");

    /// Read from the deployed DirectPay on Whitechain on Fri Nov 6 (`paymentIdFor` with these inputs).
    #[test]
    fn payment_id_matches_the_deployed_contract() {
        let id = payment_id_for(
            1874,
            DIRECT_PAY,
            b256!("abababababababababababababababababababababababababababababababab"),
            address!("5a30000000000000000000000000000000000001"),
            Amount::from(12_000_000u64),
            address!("a917000000000000000000000000000000000001"),
        );
        assert_eq!(id, b256!("dc266d741edec5cc5091738a75851f76e55aebadeaf22a15c8d52e35373afdc7"));
    }

    /// Read from the deployed DirectPay on Whitechain on Fri Nov 6 (`refundNonceFor` with these inputs).
    #[test]
    fn refund_nonce_matches_the_deployed_contract() {
        let nonce = refund_nonce_for(
            1874,
            DIRECT_PAY,
            b256!("dc266d741edec5cc5091738a75851f76e55aebadeaf22a15c8d52e35373afdc7"),
            Amount::from(5_000_000u64),
            Amount::from(2_000_000u64),
        );
        assert_eq!(nonce, b256!("5ea3cc3f7c590fb05c55c1553aac6b94f799dde5f48cee529ea4b45ddec7d817"));
    }

    sol! {
        struct ReceiveWithAuthorization {
            address from;
            address to;
            uint256 value;
            uint256 validAfter;
            uint256 validBefore;
            bytes32 nonce;
        }
    }

    /// The JSON must hash to exactly what USDT checks, or every signature would be "invalid".
    #[test]
    fn the_json_hashes_like_the_token_expects() {
        let token = Token {
            address: USDT,
            decimals: 6,
            eip712: Eip712Domain { name: "Tether USD".into(), version: "1".into() },
        };
        let note = Note {
            from: address!("a917000000000000000000000000000000000001"),
            to: DIRECT_PAY,
            value: Amount::from(12_000_000u64),
            valid_after: 1_793_913_940,
            valid_before: 1_793_914_600,
            nonce: B256::repeat_byte(0x42),
        };
        let json = typed_data(&token, 1874, &note);
        let from_json: TypedData = serde_json::from_value(json).unwrap();

        let expected = ReceiveWithAuthorization {
            from: note.from,
            to: note.to,
            value: U256::from(12_000_000u64),
            validAfter: U256::from(1_793_913_940u64),
            validBefore: U256::from(1_793_914_600u64),
            nonce: note.nonce,
        };
        let domain = eip712_domain! {
            name: "Tether USD",
            version: "1",
            chain_id: 1874,
            verifying_contract: USDT,
        };
        assert_eq!(from_json.eip712_signing_hash().unwrap(), expected.eip712_signing_hash(&domain));
    }
}
