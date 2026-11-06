//! Everything the Relayer asks of a chain, behind one trait so the job logic can be tested with a
//! fake chain and run against Whitechain (or a local anvil copy of it) unchanged.
//!
//! Signing and sending are separate steps: the worker saves the signed bytes before it sends
//! them, so a retry after a timeout or a crash sends the very same transaction again.

use std::{future::Future, time::Duration};

use alloy::{
    eips::eip2718::Encodable2718,
    network::{EthereumWallet, NetworkTransactionBuilder, TransactionBuilder},
    primitives::{Address, B256, Bytes, Signature, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::{Filter, TransactionRequest},
    signers::local::PrivateKeySigner,
    sol,
    sol_types::{SolCall, SolEvent},
    transports::{RpcError, TransportErrorKind},
};
use common::{amount::Amount, chains::Chain as ChainConfig, jobs::Job};

sol! {
    // The contract's nine arguments, not ours to shorten.
    #[allow(clippy::too_many_arguments)]
    #[sol(rpc)]
    interface IDirectPay {
        function pay(bytes32 orderId, address merchant, uint256 amount, address payer, uint256 validAfter,
                     uint256 validBefore, uint8 v, bytes32 r, bytes32 s) external returns (bytes32 paymentId);
        event Paid(bytes32 indexed paymentId, bytes32 indexed orderId, address indexed merchant,
                   address payer, uint256 amount);
    }

    #[sol(rpc)]
    interface IUsdt {
        function balanceOf(address owner) external view returns (uint256);
        function isBlocked(address account) external view returns (bool);
        function authorizationState(address authorizer, bytes32 nonce) external view returns (bool);
    }
}

/// Why a chain call failed. A revert is final for this job; anything else is worth a retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainError {
    /// The contract refused (`execution reverted: ...`). Sending would waste gas.
    Revert(String),
    /// The RPC was down, slow or returned nonsense. Try again later.
    Rpc(String),
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainError::Revert(reason) => write!(f, "reverted: {reason}"),
            ChainError::Rpc(reason) => write!(f, "rpc: {reason}"),
        }
    }
}

/// A mined transaction: who sent it, whether it succeeded and what its gas cost (wei).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mined {
    pub from: Address,
    pub success: bool,
    pub gas_used: u64,
    pub gas_cost: U256,
}

/// A signed transaction that has not necessarily been sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedTx {
    pub raw: Bytes,
    pub hash: B256,
    pub nonce: u64,
}

/// `DirectPay.pay`'s arguments, with the signature split into v, r, s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayCall {
    pub order_id: B256,
    pub merchant: Address,
    pub amount: Amount,
    pub payer: Address,
    pub valid_after: u64,
    pub valid_before: u64,
    pub nonce: B256,
    pub v: u8,
    pub r: B256,
    pub s: B256,
}

impl PayCall {
    /// Every job kind the Relayer can send is listed here; there is no way to send anything else.
    pub fn from_job(job: &Job) -> Result<PayCall, String> {
        let Job::DirectPay { order_id, merchant, amount, payer, valid_after, valid_before, nonce, signature } = job;
        let (v, r, s) = split_signature(signature)?;
        Ok(PayCall {
            order_id: *order_id,
            merchant: *merchant,
            amount: *amount,
            payer: *payer,
            valid_after: *valid_after,
            valid_before: *valid_before,
            nonce: *nonce,
            v,
            r,
            s,
        })
    }

    fn abi(&self) -> IDirectPay::payCall {
        IDirectPay::payCall {
            orderId: self.order_id,
            merchant: self.merchant,
            amount: self.amount.0,
            payer: self.payer,
            validAfter: U256::from(self.valid_after),
            validBefore: U256::from(self.valid_before),
            v: self.v,
            r: self.r,
            s: self.s,
        }
    }
}

/// 65 bytes into the `v` (27 or 28), `r`, `s` that `receiveWithAuthorization` takes.
pub fn split_signature(signature: &Bytes) -> Result<(u8, B256, B256), String> {
    let parsed = Signature::from_raw(signature).map_err(|error| format!("bad signature: {error}"))?;
    Ok((27 + u8::from(parsed.v()), parsed.r().into(), parsed.s().into()))
}

pub trait Chain {
    /// The Relayer's own address on this chain.
    fn sender(&self) -> Address;
    /// Has this EIP-3009 note been used? Only a mined transaction uses it, never a pending one.
    async fn note_used(&self, payer: Address, nonce: B256) -> Result<bool, ChainError>;
    async fn usdt_balance(&self, owner: Address) -> Result<U256, ChainError>;
    async fn is_blocked(&self, account: Address) -> Result<bool, ChainError>;
    /// `eth_call` of the exact transaction: free, and says whether sending would succeed.
    async fn dry_run(&self, call: &PayCall) -> Result<(), ChainError>;
    /// Signs `DirectPay.pay` with `nonce` (or the next free one) and the fee raised `fee_bumps`
    /// times by 25%. Sends nothing.
    async fn sign(&self, call: &PayCall, nonce: Option<u64>, fee_bumps: u32) -> Result<SignedTx, ChainError>;
    /// Sends signed bytes. Sending the same bytes again is harmless: it is the same transaction.
    async fn broadcast(&self, raw: &Bytes) -> Result<(), ChainError>;
    /// `None` while not mined yet.
    async fn receipt(&self, tx_hash: B256) -> Result<Option<Mined>, ChainError>;
    /// The transaction that emitted `Paid` for this payment id (the note's nonce), if any.
    async fn find_payment(&self, payment_id: B256) -> Result<Option<B256>, ChainError>;
}

/// A real chain over JSON-RPC, signing with the Relayer's key.
pub struct Rpc {
    provider: DynProvider,
    wallet: EthereumWallet,
    sender: Address,
    chain_id: u64,
    usdt: Address,
    direct_pay: Address,
    /// The RPC address, kept only to remove it from error text: it may hold an API key.
    url: String,
}

/// How far back to look for a `Paid` log. Notes live 10 minutes; Whitechain makes about one block
/// a second, so this is over an hour.
const PAYMENT_LOOKBACK_BLOCKS: u64 = 5_000;
/// Every RPC call gives up after this, well inside the worker's lease.
pub const RPC_TIMEOUT: Duration = Duration::from_secs(10);
/// Gas limit = estimate + 20%.
const GAS_MARGIN_PERCENT: u64 = 20;

impl Rpc {
    pub fn connect(config: &ChainConfig, rpc_url: &str, signer: PrivateKeySigner) -> Result<Rpc, String> {
        let usdt = config.tokens.get("USDT").ok_or("chains.json: no USDT on the shop chain")?.address;
        let direct_pay = config.contracts.direct_pay.ok_or("chains.json: no DirectPay on the shop chain")?;
        let sender = signer.address();
        let url = rpc_url.parse().map_err(|_| "the shop chain's RPC address is not a valid URL".to_owned())?;
        let provider = ProviderBuilder::new().connect_http(url).erased();
        Ok(Rpc {
            provider,
            wallet: EthereumWallet::from(signer),
            sender,
            chain_id: config.chain_id,
            usdt,
            direct_pay,
            url: rpc_url.trim_end_matches('/').to_owned(),
        })
    }

    /// The RPC call, with a time limit and the RPC address taken out of any error.
    async fn rpc<T>(&self, call: impl Future<Output = Result<T, RpcError<TransportErrorKind>>>) -> Result<T, ChainError> {
        timed(RPC_TIMEOUT, call).await?.map_err(|error| self.clean(rpc_error(error)))
    }

    async fn contract<T>(&self, call: impl Future<Output = Result<T, alloy::contract::Error>>) -> Result<T, ChainError> {
        let result = timed(RPC_TIMEOUT, call).await?;
        result.map_err(|error| {
            self.clean(match error {
                alloy::contract::Error::TransportError(error) => rpc_error(error),
                other => ChainError::Rpc(other.to_string()),
            })
        })
    }

    fn clean(&self, error: ChainError) -> ChainError {
        let scrub = |text: String| if self.url.is_empty() { text } else { text.replace(&self.url, "<rpc>") };
        match error {
            ChainError::Revert(text) => ChainError::Revert(scrub(text)),
            ChainError::Rpc(text) => ChainError::Rpc(scrub(text)),
        }
    }
}

impl Chain for Rpc {
    fn sender(&self) -> Address {
        self.sender
    }

    async fn note_used(&self, payer: Address, nonce: B256) -> Result<bool, ChainError> {
        let usdt = IUsdt::new(self.usdt, &self.provider);
        self.contract(async { usdt.authorizationState(payer, nonce).call().await }).await
    }

    async fn usdt_balance(&self, owner: Address) -> Result<U256, ChainError> {
        let usdt = IUsdt::new(self.usdt, &self.provider);
        self.contract(async { usdt.balanceOf(owner).call().await }).await
    }

    async fn is_blocked(&self, account: Address) -> Result<bool, ChainError> {
        let usdt = IUsdt::new(self.usdt, &self.provider);
        self.contract(async { usdt.isBlocked(account).call().await }).await
    }

    async fn dry_run(&self, call: &PayCall) -> Result<(), ChainError> {
        let direct_pay = IDirectPay::new(self.direct_pay, &self.provider);
        let abi = call.abi();
        self.contract(async { direct_pay.call_builder(&abi).from(self.sender).call().await.map(|_| ()) }).await
    }

    async fn sign(&self, call: &PayCall, nonce: Option<u64>, fee_bumps: u32) -> Result<SignedTx, ChainError> {
        let nonce = match nonce {
            Some(nonce) => nonce,
            None => self.rpc(self.provider.get_transaction_count(self.sender).pending().into_future()).await?,
        };
        let fees = self.rpc(self.provider.estimate_eip1559_fees()).await?;
        let (mut max_fee, mut tip) = (fees.max_fee_per_gas, fees.max_priority_fee_per_gas);
        for _ in 0..fee_bumps {
            // +25% each time; nodes replace a pending transaction only for at least +10%.
            max_fee = max_fee * 5 / 4 + 1;
            tip = tip * 5 / 4 + 1;
        }
        let mut request = TransactionRequest::default()
            .with_from(self.sender)
            .with_to(self.direct_pay)
            .with_input(call.abi().abi_encode())
            .with_chain_id(self.chain_id)
            .with_nonce(nonce)
            .with_max_fee_per_gas(max_fee)
            .with_max_priority_fee_per_gas(tip);
        let gas = self.rpc(self.provider.estimate_gas(request.clone()).into_future()).await?;
        request.set_gas_limit(gas + gas * GAS_MARGIN_PERCENT / 100);
        let envelope = request.build(&self.wallet).await.map_err(|error| ChainError::Rpc(format!("signing: {error}")))?;
        Ok(SignedTx { raw: envelope.encoded_2718().into(), hash: *envelope.tx_hash(), nonce })
    }

    async fn broadcast(&self, raw: &Bytes) -> Result<(), ChainError> {
        match self.rpc(async { self.provider.send_raw_transaction(raw).await.map(|_| ()) }).await {
            // The node already has it, or its nonce is used (by this transaction, mined, or by
            // another one): either way the receipt and the note decide what happened.
            Err(ChainError::Rpc(text) | ChainError::Revert(text)) if already_handled(&text) => Ok(()),
            // A node never "reverts" a raw send; any refusal here is worth a retry.
            Err(ChainError::Revert(text)) => Err(ChainError::Rpc(text)),
            other => other,
        }
    }

    async fn receipt(&self, tx_hash: B256) -> Result<Option<Mined>, ChainError> {
        let receipt = self.rpc(self.provider.get_transaction_receipt(tx_hash).into_future()).await?;
        Ok(receipt.map(|receipt| Mined {
            from: receipt.from,
            success: receipt.status(),
            gas_used: receipt.gas_used,
            gas_cost: U256::from(receipt.gas_used) * U256::from(receipt.effective_gas_price),
        }))
    }

    async fn find_payment(&self, payment_id: B256) -> Result<Option<B256>, ChainError> {
        let latest = self.rpc(self.provider.get_block_number().into_future()).await?;
        let filter = Filter::new()
            .address(self.direct_pay)
            .event_signature(IDirectPay::Paid::SIGNATURE_HASH)
            .topic1(payment_id)
            .from_block(latest.saturating_sub(PAYMENT_LOOKBACK_BLOCKS))
            .to_block(latest);
        let logs = self.rpc(self.provider.get_logs(&filter).into_future()).await?;
        Ok(logs.first().and_then(|log| log.transaction_hash))
    }
}

/// Runs `call`, or gives up with an RPC error after `limit`.
pub async fn timed<T>(limit: Duration, call: impl Future<Output = T>) -> Result<T, ChainError> {
    tokio::time::timeout(limit, call).await.map_err(|_| ChainError::Rpc(format!("no answer within {limit:?}")))
}

fn already_handled(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    ["already known", "known transaction", "nonce too low", "already imported"].iter().any(|phrase| text.contains(phrase))
}

fn rpc_error(error: RpcError<TransportErrorKind>) -> ChainError {
    match error.as_error_resp() {
        // JSON-RPC code 3 is "execution reverted".
        Some(response) if response.code == 3 || response.message.contains("revert") => {
            ChainError::Revert(response.message.to_string())
        }
        _ => ChainError::Rpc(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::SignerSync;

    #[test]
    fn signatures_split_into_v_27_or_28() {
        let signer = PrivateKeySigner::random();
        for byte in 0..8u8 {
            let signature = signer.sign_hash_sync(&B256::repeat_byte(byte)).unwrap();
            let (v, r, s) = split_signature(&Bytes::from(signature.as_bytes().to_vec())).unwrap();
            assert!(v == 27 || v == 28, "v = {v}");
            assert_eq!((r, s), (signature.r().into(), signature.s().into()));
        }
        assert!(split_signature(&Bytes::from(vec![1u8; 64])).is_err());
    }

    #[tokio::test]
    async fn a_call_that_never_answers_times_out() {
        let result = timed(Duration::from_millis(20), std::future::pending::<()>()).await;
        assert!(matches!(result, Err(ChainError::Rpc(_))));
    }

    #[test]
    fn errors_never_carry_the_rpc_address() {
        let config = common::chains::Chains::from_json(include_str!("../../../shared/chains.json")).unwrap();
        let url = "https://rpc.example/v1/SECRET-KEY";
        let rpc = Rpc::connect(config.shop_chain(), url, PrivateKeySigner::random()).unwrap();
        let cleaned = rpc.clean(ChainError::Rpc(format!("error sending request for url ({url}/)")));
        assert_eq!(cleaned, ChainError::Rpc("error sending request for url (<rpc>/)".into()));
    }

    #[test]
    fn resending_known_transactions_counts_as_sent() {
        for text in ["already known", "Known transaction: 0xab", "nonce too low: next nonce 5, tx nonce 4"] {
            assert!(already_handled(text), "{text}");
        }
        assert!(!already_handled("insufficient funds for gas * price + value"));
    }
}
