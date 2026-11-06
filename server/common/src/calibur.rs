//! Gas-free locks of any ERC-20 through Uniswap's Calibur, an audited EIP-7702 delegate.
//!
//! A token without EIP-3009 (WETH, WBTC, ...) cannot be pulled with a signed note. Instead the
//! customer's wallet delegates to Calibur once (an EIP-7702 authorization), and for each payment
//! signs one `SignedBatchedCall` with two calls: `token.approve(sourceLock, amount)` and
//! `sourceLock.lock(params, token, amount)`. The Gas Payer submits it as
//! `customer.execute(signedBatchedCall, wrappedSignature)` and pays the gas. Inside the batch the
//! customer's own account is the caller, so it is the lock's payer.
//!
//! Both the Order Desk (what the wallet signs) and the Gas Payer (what it sends) build the batch
//! here, from the same lock, so they cannot disagree.

use alloy::{
    dyn_abi::Eip712Domain,
    primitives::{keccak256, Address, Bytes, B256, U256},
    sol,
    sol_types::{SolCall, SolStruct, SolValue},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{amount::Amount, locks::SourceLockParams};

/// Calibur's EIP-712 domain name and version (`eip712Domain()` on the deployed v1.1.0).
pub const DOMAIN_NAME: &str = "Calibur";
pub const DOMAIN_VERSION: &str = "1.0.0";

sol! {
    struct Call {
        address to;
        uint256 value;
        bytes data;
    }

    struct BatchedCall {
        Call[] calls;
        bool revertOnFailure;
    }

    struct SignedBatchedCall {
        BatchedCall batchedCall;
        uint256 nonce;
        bytes32 keyHash;
        address executor;
        uint256 deadline;
    }

    /// The parts of Calibur the Gas Payer calls.
    interface ICalibur {
        function execute(SignedBatchedCall signedBatchedCall, bytes wrappedSignature) external payable;
    }

    interface IErc20Approve {
        function approve(address spender, uint256 amount) external returns (bool);
    }

    interface ISourceLockLock {
        struct LockParams {
            bytes32 orderId;
            address receiver;
            uint256 minUsdcOut;
            bytes32 hashlock;
            uint64 expiry;
            address outToken;
        }

        function lock(LockParams p, address token, uint256 amount) external returns (bytes32 lockId);
    }
}

/// An EIP-7702 authorization the customer's wallet signed: "run Calibur's code at my address".
/// Sent once, with the first batch; after that the wallet is delegated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Authorization {
    pub chain_id: u64,
    /// The delegate: must be Calibur.
    pub address: Address,
    /// The wallet's transaction nonce when it signed.
    pub nonce: u64,
    pub y_parity: u8,
    pub r: U256,
    pub s: U256,
}

/// What a batch lock carries besides the lock itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    /// Calibur nonce: `nonce_for(lock_id)`.
    pub nonce: U256,
    /// Unix seconds after which Calibur refuses the signature.
    pub deadline: u64,
    /// Present when the wallet is not delegated to Calibur yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<Authorization>,
}

/// The Calibur nonce for a lock: its own nonce key (from the lock id) at sequence 0. Each lock id
/// is used once, so no on-chain read is needed, and a signature works for this lock only.
pub fn nonce_for(lock_id: B256) -> U256 {
    let key = U256::from_be_bytes(keccak256(lock_id).0) >> 64;
    key << 64
}

fn lock_params(params: &SourceLockParams) -> ISourceLockLock::LockParams {
    let p = params.to_sol();
    ISourceLockLock::LockParams {
        orderId: p.orderId,
        receiver: p.receiver,
        minUsdcOut: p.minUsdcOut,
        hashlock: p.hashlock,
        expiry: p.expiry,
        outToken: p.outToken,
    }
}

/// The two calls of a lock: approve exactly the amount to SourceLock, then lock it.
pub fn lock_calls(
    source_lock: Address,
    params: &SourceLockParams,
    token: Address,
    amount: Amount,
) -> Vec<Call> {
    let approve = IErc20Approve::approveCall {
        spender: source_lock,
        amount: amount.0,
    }
    .abi_encode();
    let lock = ISourceLockLock::lockCall {
        p: lock_params(params),
        token,
        amount: amount.0,
    }
    .abi_encode();
    vec![
        Call {
            to: token,
            value: U256::ZERO,
            data: approve.into(),
        },
        Call {
            to: source_lock,
            value: U256::ZERO,
            data: lock.into(),
        },
    ]
}

/// The signed batch: all calls must succeed, signed by the wallet's own key (`keyHash` 0), and
/// anyone may submit it (`executor` 0): the calls are fixed, so a front-runner only does the
/// same lock.
pub fn signed_batch(calls: Vec<Call>, nonce: U256, deadline: u64) -> SignedBatchedCall {
    SignedBatchedCall {
        batchedCall: BatchedCall {
            calls,
            revertOnFailure: true,
        },
        nonce,
        keyHash: B256::ZERO,
        executor: Address::ZERO,
        deadline: U256::from(deadline),
    }
}

/// Calibur's domain at the customer's address: `verifyingContract` is the wallet itself, and the
/// salt is the implementation (Calibur) address with a zero prefix.
pub fn domain(chain_id: u64, account: Address, calibur: Address) -> Eip712Domain {
    Eip712Domain {
        name: Some(DOMAIN_NAME.into()),
        version: Some(DOMAIN_VERSION.into()),
        chain_id: Some(U256::from(chain_id)),
        verifying_contract: Some(account),
        salt: Some(calibur.into_word()),
    }
}

/// The digest the wallet signs.
pub fn signing_hash(
    batch: &SignedBatchedCall,
    chain_id: u64,
    account: Address,
    calibur: Address,
) -> B256 {
    batch.eip712_signing_hash(&domain(chain_id, account, calibur))
}

/// The EIP-712 JSON for `eth_signTypedData_v4` (numbers as decimal text, bytes as 0x hex).
pub fn typed_data(
    batch: &SignedBatchedCall,
    chain_id: u64,
    account: Address,
    calibur: Address,
) -> Value {
    let calls: Vec<Value> = batch
        .batchedCall
        .calls
        .iter()
        .map(|call| json!({ "to": format!("{:#x}", call.to), "value": call.value.to_string(), "data": call.data.to_string() }))
        .collect();
    json!({
        "domain": {
            "name": DOMAIN_NAME,
            "version": DOMAIN_VERSION,
            "chainId": chain_id,
            "verifyingContract": format!("{account:#x}"),
            "salt": calibur.into_word().to_string(),
        },
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "version", "type": "string" },
                { "name": "chainId", "type": "uint256" },
                { "name": "verifyingContract", "type": "address" },
                { "name": "salt", "type": "bytes32" },
            ],
            "SignedBatchedCall": [
                { "name": "batchedCall", "type": "BatchedCall" },
                { "name": "nonce", "type": "uint256" },
                { "name": "keyHash", "type": "bytes32" },
                { "name": "executor", "type": "address" },
                { "name": "deadline", "type": "uint256" },
            ],
            "BatchedCall": [
                { "name": "calls", "type": "Call[]" },
                { "name": "revertOnFailure", "type": "bool" },
            ],
            "Call": [
                { "name": "to", "type": "address" },
                { "name": "value", "type": "uint256" },
                { "name": "data", "type": "bytes" },
            ],
        },
        "primaryType": "SignedBatchedCall",
        "message": {
            "batchedCall": { "calls": calls, "revertOnFailure": batch.batchedCall.revertOnFailure },
            "nonce": batch.nonce.to_string(),
            "keyHash": batch.keyHash.to_string(),
            "executor": format!("{:#x}", batch.executor),
            "deadline": batch.deadline.to_string(),
        },
    })
}

/// `execute`'s second argument: `abi.encode(signature, hookData)` with no hook data.
pub fn wrapped_signature(signature: &Bytes) -> Bytes {
    (signature.clone(), Bytes::new()).abi_encode_params().into()
}

/// The calldata of `customer.execute(batch, wrappedSignature)`.
pub fn execute_calldata(batch: SignedBatchedCall, signature: &Bytes) -> Bytes {
    ICalibur::executeCall {
        signedBatchedCall: batch,
        wrappedSignature: wrapped_signature(signature),
    }
    .abi_encode()
    .into()
}

/// The code of a wallet delegated to `delegate` under EIP-7702: `0xef0100 ‖ address`.
pub fn delegation_code(delegate: Address) -> Bytes {
    let mut code = vec![0xef, 0x01, 0x00];
    code.extend_from_slice(delegate.as_slice());
    code.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        dyn_abi::TypedData,
        primitives::address,
        signers::{local::PrivateKeySigner, SignerSync},
    };

    const CALIBUR: Address = address!("000000005c84F8Fd50b21CAC312528A64437030e");
    const LOCK: Address = address!("3aCDE874Eeb36dD28507B5b1FF29219Ba262F5d9");
    const WBTC: Address = address!("29f2D40B0605204364af54EC677bD022dA425d03");

    fn params() -> SourceLockParams {
        SourceLockParams {
            order_id: B256::repeat_byte(1),
            receiver: Address::repeat_byte(0xb0),
            min_usdc_out: Amount::from(1_003_000u64),
            hashlock: B256::repeat_byte(2),
            expiry: 1_800_000_000,
            out_token: Address::ZERO,
        }
    }

    #[test]
    fn the_wallet_json_hashes_to_the_same_digest() {
        let account = Address::repeat_byte(0xaa);
        let batch = signed_batch(
            lock_calls(LOCK, &params(), WBTC, Amount::from(1_500u64)),
            nonce_for(B256::repeat_byte(3)),
            1_800_000_100,
        );
        let json = typed_data(&batch, 11_155_111, account, CALIBUR);
        let typed: TypedData = serde_json::from_value(json).unwrap();
        assert_eq!(
            typed.eip712_signing_hash().unwrap(),
            signing_hash(&batch, 11_155_111, account, CALIBUR)
        );
    }

    #[test]
    fn a_nonce_is_sequence_zero_of_its_own_key() {
        let nonce = nonce_for(B256::repeat_byte(3));
        assert_eq!(nonce & U256::from(u64::MAX), U256::ZERO);
        assert_ne!(nonce, U256::ZERO);
        assert_ne!(nonce, nonce_for(B256::repeat_byte(4)));
    }

    #[test]
    fn the_signature_recovers_to_the_wallet() {
        let wallet = PrivateKeySigner::random();
        let batch = signed_batch(
            lock_calls(LOCK, &params(), WBTC, Amount::from(1_500u64)),
            nonce_for(B256::repeat_byte(3)),
            1_800_000_100,
        );
        let hash = signing_hash(&batch, 84_532, wallet.address(), CALIBUR);
        let signature = wallet.sign_hash_sync(&hash).unwrap();
        assert_eq!(
            signature.recover_address_from_prehash(&hash).unwrap(),
            wallet.address()
        );
    }

    #[test]
    fn delegation_code_is_the_7702_designator() {
        let code = delegation_code(CALIBUR);
        assert_eq!(code.len(), 23);
        assert_eq!(&code[..3], &[0xef, 0x01, 0x00]);
    }
}
