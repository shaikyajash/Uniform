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
use common::{
    amount::Amount,
    chains::Chain as ChainConfig,
    jobs::Job,
    locks::{SourceLockParams, hashlock_of, source_lock_id},
};

sol! {
    // The contract's nine arguments, not ours to shorten.
    #[allow(clippy::too_many_arguments)]
    #[sol(rpc)]
    interface IDirectPay {
        function pay(bytes32 orderId, address merchant, uint256 amount, address payer, uint256 validAfter,
                     uint256 validBefore, uint8 v, bytes32 r, bytes32 s) external returns (bytes32 paymentId);
        event Paid(bytes32 indexed paymentId, bytes32 indexed orderId, address indexed merchant,
                   address payer, uint256 amount);
        function refund(bytes32 paymentId, uint256 amount, uint256 validAfter, uint256 validBefore, uint8 v,
                        bytes32 r, bytes32 s) external;
        function payments(bytes32 paymentId) external view returns (address payer, uint96 amount,
            address merchant, uint96 refunded, bytes32 orderId);
        function refundNonceFor(bytes32 paymentId, uint256 amount, uint256 refundedBefore)
            external view returns (bytes32);
        event Refunded(bytes32 indexed paymentId, bytes32 indexed orderId, address indexed payer, uint256 amount,
                       uint256 totalRefunded);
    }

    /// USDT on Whitechain, and USDC or EURC on customer chains: the EIP-3009 parts.
    #[sol(rpc)]
    interface IToken {
        function balanceOf(address owner) external view returns (uint256);
        function authorizationState(address authorizer, bytes32 nonce) external view returns (bool);
    }

    /// Tether's block list (USDT).
    #[sol(rpc)]
    interface ITether {
        function isBlocked(address account) external view returns (bool);
    }

    /// Circle's block list (USDC, EURC).
    #[sol(rpc)]
    interface ICircle {
        function isBlacklisted(address account) external view returns (bool);
    }

    struct LockParams {
        bytes32 orderId;
        address receiver;
        uint256 minUsdcOut;
        bytes32 hashlock;
        uint64 expiry;
        address outToken;
    }

    #[allow(clippy::too_many_arguments)]
    #[sol(rpc)]
    interface ISourceLock {
        function lockWithAuthorization(LockParams p, address payer, address tokenIn, uint256 amountIn,
                                       uint256 validAfter, uint256 validBefore, uint8 v, bytes32 r, bytes32 s,
                                       bytes swapData) external returns (bytes32 lockId);
        function refund(bytes32 lockId) external;
        function locks(bytes32 lockId) external view returns (address payer, uint64 expiry, uint8 state,
            address receiver, uint96 amount, bytes32 hashlock, bytes32 orderId, address token);
        event Locked(bytes32 indexed lockId, bytes32 indexed orderId, address payer, address receiver,
                     address token, uint256 amount, bytes32 hashlock, uint64 expiry);
        event Refunded(bytes32 indexed lockId);
    }

    #[sol(rpc)]
    interface IDestinationLock {
        function claim(bytes32 lockId, bytes32 secret) external;
        function locks(bytes32 lockId) external view returns (address maker, uint64 expiry, uint8 state,
            address merchant, uint96 amount, bytes32 hashlock, bytes32 orderId, bytes32 strategyHash,
            bytes32 srcLockId, uint64 srcChainId);
        event Settled(bytes32 indexed lockId, bytes32 indexed orderId, address indexed merchant, uint256 amount,
                      uint256 srcChainId, bytes32 srcLockId, bytes32 secret);
    }
}

/// A lock's state in both lock contracts.
const LOCK_OPEN: u8 = 1;
const LOCK_CLAIMED: u8 = 2;
const LOCK_REFUNDED: u8 = 3;

/// Why a chain call failed. A revert is final for this job; anything else is worth a retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainError {
    /// The contract (or one of our checks) refused. Sending would waste gas.
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

fn refuse<T>(reason: impl Into<String>) -> Result<T, ChainError> {
    Err(ChainError::Revert(reason.into()))
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

/// `SourceLock.lockWithAuthorization`'s arguments, with the signature split into v, r, s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockCall {
    pub params: SourceLockParams,
    pub payer: Address,
    pub token: Address,
    pub amount: Amount,
    pub valid_after: u64,
    pub valid_before: u64,
    pub lock_id: B256,
    pub v: u8,
    pub r: B256,
    pub s: B256,
}

/// `DirectPay.refund`'s arguments, from the note the shop signed, with what the note was made against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefundCall {
    pub payment_id: B256,
    pub merchant: Address,
    pub amount: Amount,
    pub refunded_before: Amount,
    pub valid_after: u64,
    pub valid_before: u64,
    /// `refundNonceFor(paymentId, amount, refundedBefore)`: the note's nonce.
    pub nonce: B256,
    pub v: u8,
    pub r: B256,
    pub s: B256,
}

/// `DestinationLock.claim`, and what the lock must promise before the secret is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCall {
    pub lock_id: B256,
    pub secret: B256,
    pub merchant: Address,
    pub min_amount: Amount,
    pub deadline: u64,
}

/// One transaction the Relayer can send. Every job kind maps to exactly one of these; there is no
/// way to send anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    DirectPay(PayCall),
    SourceLock(LockCall),
    DestinationClaim(ClaimCall),
    SourceRefund { lock_id: B256 },
    DirectRefund(RefundCall),
}

impl Call {
    pub fn from_job(job: &Job) -> Result<Call, String> {
        Ok(match job {
            Job::DirectPay { order_id, merchant, amount, payer, valid_after, valid_before, nonce, signature } => {
                let (v, r, s) = split_signature(signature)?;
                Call::DirectPay(PayCall {
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
            Job::SourceLock { params, payer, token, amount, valid_after, valid_before, lock_id, signature } => {
                let (v, r, s) = split_signature(signature)?;
                Call::SourceLock(LockCall {
                    params: *params,
                    payer: *payer,
                    token: *token,
                    amount: *amount,
                    valid_after: *valid_after,
                    valid_before: *valid_before,
                    lock_id: *lock_id,
                    v,
                    r,
                    s,
                })
            }
            Job::DestinationClaim { lock_id, secret, merchant, min_amount, deadline } => {
                Call::DestinationClaim(ClaimCall {
                    lock_id: *lock_id,
                    secret: *secret,
                    merchant: *merchant,
                    min_amount: *min_amount,
                    deadline: *deadline,
                })
            }
            Job::SourceRefund { lock_id } => Call::SourceRefund { lock_id: *lock_id },
            Job::DirectRefund {
                order_id: _,
                payment_id,
                merchant,
                amount,
                refunded_before,
                valid_after,
                valid_before,
                nonce,
                signature,
            } => {
                let (v, r, s) = split_signature(signature)?;
                Call::DirectRefund(RefundCall {
                    payment_id: *payment_id,
                    merchant: *merchant,
                    amount: *amount,
                    refunded_before: *refunded_before,
                    valid_after: *valid_after,
                    valid_before: *valid_before,
                    nonce: *nonce,
                    v,
                    r,
                    s,
                })
            }
        })
    }

    /// What marks this call as done on chain, by anyone: the note's nonce (a payment, a lock or a
    /// shop refund), or the lock id (a claim or a lock refund).
    pub fn done_id(&self) -> B256 {
        match self {
            Call::DirectPay(call) => call.nonce,
            Call::SourceLock(call) => call.lock_id,
            Call::DestinationClaim(call) => call.lock_id,
            Call::SourceRefund { lock_id } => *lock_id,
            Call::DirectRefund(call) => call.nonce,
        }
    }

    /// The first indexed topic of the log this call emits. The done id, except for a shop refund,
    /// whose `Refunded` log is under the payment id.
    pub fn log_topic(&self) -> B256 {
        match self {
            Call::DirectRefund(call) => call.payment_id,
            _ => self.done_id(),
        }
    }

    /// After this (unix seconds) the call can no longer succeed. A refund never runs out.
    pub fn deadline(&self) -> Option<u64> {
        match self {
            Call::DirectPay(call) => Some(call.valid_before),
            Call::SourceLock(call) => Some(call.valid_before),
            Call::DestinationClaim(call) => Some(call.deadline),
            Call::SourceRefund { .. } => None,
            Call::DirectRefund(call) => Some(call.valid_before),
        }
    }

    /// Whether the shop's daily gas budget can stop this call. It can stop a call that starts a
    /// payment, while the customer's money is still in their wallet, and a shop's own refund. A
    /// claim or a lock refund finishes money already locked, so it is sent even when the shop has
    /// spent its budget.
    pub fn uses_shop_budget(&self) -> bool {
        matches!(self, Call::DirectPay(_) | Call::SourceLock(_) | Call::DirectRefund(_))
    }

    /// The amount checked against the Relayer's order limit, in 6-decimal units. A lock carries
    /// Bob's fee on top of the order, so it is allowed 1% more.
    pub fn amount_for_limit(&self) -> Option<U256> {
        match self {
            Call::DirectPay(call) => Some(call.amount.0),
            Call::SourceLock(call) => Some(call.amount.0 * U256::from(100u64) / U256::from(101u64)),
            Call::DestinationClaim(_) | Call::SourceRefund { .. } | Call::DirectRefund(_) => None,
        }
    }

    /// Whether a failure that leaves nothing on chain hands the order back to `quoted`, so the
    /// customer can sign again. Only the calls that carry the customer's note do.
    pub fn reopens_order(&self) -> bool {
        matches!(self, Call::DirectPay(_) | Call::SourceLock(_))
    }

    /// For messages: what runs out at the deadline.
    pub fn what_runs_out(&self) -> &'static str {
        match self {
            Call::DirectPay(_) | Call::SourceLock(_) | Call::DirectRefund(_) => "the note",
            Call::DestinationClaim(_) | Call::SourceRefund { .. } => "the lock",
        }
    }
}

impl PayCall {
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

impl RefundCall {
    fn abi(&self) -> IDirectPay::refundCall {
        IDirectPay::refundCall {
            paymentId: self.payment_id,
            amount: self.amount.0,
            validAfter: U256::from(self.valid_after),
            validBefore: U256::from(self.valid_before),
            v: self.v,
            r: self.r,
            s: self.s,
        }
    }
}

impl LockCall {
    fn abi(&self) -> ISourceLock::lockWithAuthorizationCall {
        let p = self.params;
        ISourceLock::lockWithAuthorizationCall {
            p: LockParams {
                orderId: p.order_id,
                receiver: p.receiver,
                minUsdcOut: p.min_usdc_out.0,
                hashlock: p.hashlock,
                expiry: p.expiry,
                outToken: p.out_token,
            },
            payer: self.payer,
            tokenIn: self.token,
            amountIn: self.amount.0,
            validAfter: U256::from(self.valid_after),
            validBefore: U256::from(self.valid_before),
            v: self.v,
            r: self.r,
            s: self.s,
            swapData: Bytes::new(),
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
    /// Has this call's effect happened on chain, by anyone? A note is used, or a lock claimed or
    /// refunded, only by a mined transaction, never by a pending one.
    async fn done(&self, call: &Call) -> Result<bool, ChainError>;
    /// Every check before signing, ending with a dry run (`eth_call` of the exact transaction).
    /// `Revert` means sending would fail or do harm; `Rpc` means try again.
    async fn check(&self, call: &Call, now: u64) -> Result<(), ChainError>;
    /// Signs the call with `nonce` (or the next free one) and the fee raised `fee_bumps` times by
    /// 25%. Sends nothing.
    async fn sign(&self, call: &Call, nonce: Option<u64>, fee_bumps: u32) -> Result<SignedTx, ChainError>;
    /// Sends signed bytes. Sending the same bytes again is harmless: it is the same transaction.
    async fn broadcast(&self, raw: &Bytes) -> Result<(), ChainError>;
    /// `None` while not mined yet.
    async fn receipt(&self, tx_hash: B256) -> Result<Option<Mined>, ChainError>;
    /// The transaction that did this call's effect (its log), if any.
    async fn find_done(&self, call: &Call) -> Result<Option<B256>, ChainError>;
}

/// Our contracts and tokens on one chain, from chains.json. A call for a contract that is not on
/// this chain is refused.
#[derive(Debug, Clone, Default)]
struct Contracts {
    direct_pay: Option<Address>,
    source_lock: Option<Address>,
    destination_lock: Option<Address>,
    usdt: Option<Address>,
    /// The tokens a SourceLock here may hold for us (USDC, EURC).
    lockable: Vec<Address>,
}

/// A real chain over JSON-RPC, signing with the Relayer's key.
pub struct Rpc {
    provider: DynProvider,
    wallet: EthereumWallet,
    sender: Address,
    chain_id: u64,
    contracts: Contracts,
    /// The RPC address, kept only to remove it from error text: it may hold an API key.
    url: String,
}

/// How far back to look for the log that shows a call was done. Notes live 10 minutes and locks
/// 20; Whitechain makes about one block a second and Base one every two, so this is over an hour.
const LOG_LOOKBACK_BLOCKS: u64 = 5_000;
/// Every RPC call gives up after this, well inside the worker's lease.
pub const RPC_TIMEOUT: Duration = Duration::from_secs(10);
/// Gas limit = estimate + 20%.
const GAS_MARGIN_PERCENT: u64 = 20;
/// SPEC: do not send a claim with less than this left on the destination lock. A claim that
/// reverts still puts the secret on chain, where Bob could use it.
const MIN_CLAIM_TIME_LEFT: u64 = 120;
/// Do not lock a customer's tokens unless Bob still has this long to fill: chains.json
/// `fillCutoff` (900 s) plus a minute for the transaction and Base's safe block.
pub const MIN_LOCK_TIME_LEFT: u64 = 960;

impl Rpc {
    pub fn connect(config: &ChainConfig, rpc_url: &str, signer: PrivateKeySigner) -> Result<Rpc, String> {
        let token = |symbol: &str| config.tokens.get(symbol).map(|token| token.address);
        let contracts = Contracts {
            direct_pay: config.contracts.direct_pay,
            source_lock: config.contracts.source_lock,
            destination_lock: config.contracts.destination_lock,
            usdt: token("USDT"),
            lockable: ["USDC", "EURC"].into_iter().filter_map(token).collect(),
        };
        let sender = signer.address();
        let url = rpc_url.parse().map_err(|_| format!("chain {}: the RPC address is not a valid URL", config.chain_id))?;
        let provider = ProviderBuilder::new().connect_http(url).erased();
        Ok(Rpc {
            provider,
            wallet: EthereumWallet::from(signer),
            sender,
            chain_id: config.chain_id,
            contracts,
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

    fn need(&self, address: Option<Address>, what: &str) -> Result<Address, ChainError> {
        address.map_or_else(|| refuse(format!("no {what} on chain {}", self.chain_id)), Ok)
    }

    /// Where the call goes and its calldata.
    fn target(&self, call: &Call) -> Result<(Address, Vec<u8>), ChainError> {
        Ok(match call {
            Call::DirectPay(call) => (self.need(self.contracts.direct_pay, "DirectPay")?, call.abi().abi_encode()),
            Call::SourceLock(call) => (self.need(self.contracts.source_lock, "SourceLock")?, call.abi().abi_encode()),
            Call::DestinationClaim(call) => (
                self.need(self.contracts.destination_lock, "DestinationLock")?,
                IDestinationLock::claimCall { lockId: call.lock_id, secret: call.secret }.abi_encode(),
            ),
            Call::SourceRefund { lock_id } => (
                self.need(self.contracts.source_lock, "SourceLock")?,
                ISourceLock::refundCall { lockId: *lock_id }.abi_encode(),
            ),
            Call::DirectRefund(call) => (self.need(self.contracts.direct_pay, "DirectPay")?, call.abi().abi_encode()),
        })
    }

    /// `eth_call` of the exact transaction: free, and says whether sending would succeed.
    async fn dry_run(&self, call: &Call) -> Result<(), ChainError> {
        let (to, input) = self.target(call)?;
        let request = TransactionRequest::default().with_from(self.sender).with_to(to).with_input(input);
        self.rpc(self.provider.call(request).into_future()).await.map(|_| ())
    }

    async fn note_used(&self, token: Address, payer: Address, nonce: B256) -> Result<bool, ChainError> {
        let token = IToken::new(token, &self.provider);
        self.contract(async { token.authorizationState(payer, nonce).call().await }).await
    }

    async fn balance(&self, token: Address, owner: Address) -> Result<U256, ChainError> {
        let token = IToken::new(token, &self.provider);
        self.contract(async { token.balanceOf(owner).call().await }).await
    }

    async fn source_lock_state(&self, lock_id: B256) -> Result<ISourceLock::locksReturn, ChainError> {
        let lock = ISourceLock::new(self.need(self.contracts.source_lock, "SourceLock")?, &self.provider);
        self.contract(async { lock.locks(lock_id).call().await }).await
    }

    async fn destination_lock_state(&self, lock_id: B256) -> Result<IDestinationLock::locksReturn, ChainError> {
        let lock = IDestinationLock::new(self.need(self.contracts.destination_lock, "DestinationLock")?, &self.provider);
        self.contract(async { lock.locks(lock_id).call().await }).await
    }

    async fn check_pay(&self, call: &PayCall) -> Result<(), ChainError> {
        let usdt = self.need(self.contracts.usdt, "USDT")?;
        let tether = ITether::new(usdt, &self.provider);
        if self.contract(async { tether.isBlocked(call.payer).call().await }).await? {
            return refuse("the payer is blocked by USDT");
        }
        if self.balance(usdt, call.payer).await? < call.amount.0 {
            return refuse("the payer does not have enough USDT");
        }
        Ok(())
    }

    async fn check_lock(&self, call: &LockCall, now: u64) -> Result<(), ChainError> {
        let source_lock = self.need(self.contracts.source_lock, "SourceLock")?;
        if !self.contracts.lockable.contains(&call.token) {
            return refuse("the token cannot be locked on this chain");
        }
        let expected = source_lock_id(self.chain_id, source_lock, &call.params, call.payer, call.token, call.amount);
        if expected != call.lock_id {
            return refuse("the lock id does not match its parameters");
        }
        if call.params.expiry < now + MIN_LOCK_TIME_LEFT {
            return refuse("too little time left on the lock for Bob to fill it");
        }
        let circle = ICircle::new(call.token, &self.provider);
        if self.contract(async { circle.isBlacklisted(call.payer).call().await }).await? {
            return refuse("the payer is blacklisted by Circle");
        }
        if self.balance(call.token, call.payer).await? < call.amount.0 {
            return refuse("the payer does not have enough USDC");
        }
        Ok(())
    }

    /// SPEC: before sending the secret, the lock must pay this shop at least the order amount,
    /// under this secret, with time to spare, and Tether must not have blocked the lock.
    async fn check_claim(&self, call: &ClaimCall, now: u64) -> Result<(), ChainError> {
        let lock = self.destination_lock_state(call.lock_id).await?;
        if lock.state != LOCK_OPEN {
            return refuse("the shop's lock is not open");
        }
        if lock.merchant != call.merchant {
            return refuse("the shop's lock pays someone else");
        }
        if U256::from(lock.amount) < call.min_amount.0 {
            return refuse("the shop's lock holds less than the order");
        }
        if lock.hashlock != hashlock_of(call.secret) {
            return refuse("the shop's lock is under another secret");
        }
        if lock.expiry < now + MIN_CLAIM_TIME_LEFT {
            return refuse("too little time left on the shop's lock to send the secret safely");
        }
        let usdt = self.need(self.contracts.usdt, "USDT")?;
        let destination_lock = self.need(self.contracts.destination_lock, "DestinationLock")?;
        let tether = ITether::new(usdt, &self.provider);
        if self.contract(async { tether.isBlocked(destination_lock).call().await }).await? {
            return refuse("USDT has blocked the DestinationLock");
        }
        Ok(())
    }

    /// A shop refund only for the payment's own shop, against the refunds DirectPay has now, with
    /// the nonce DirectPay itself computes, and only if the shop holds the USDT.
    async fn check_direct_refund(&self, call: &RefundCall) -> Result<(), ChainError> {
        let direct_pay = IDirectPay::new(self.need(self.contracts.direct_pay, "DirectPay")?, &self.provider);
        let payment = self.contract(async { direct_pay.payments(call.payment_id).call().await }).await?;
        if payment.payer == Address::ZERO {
            return refuse("DirectPay has no such payment");
        }
        if payment.merchant != call.merchant {
            return refuse("the payment went to another shop");
        }
        if U256::from(payment.refunded) != call.refunded_before.0 {
            return refuse("another refund landed first; the shop must sign a new note");
        }
        if call.amount.0 > U256::from(payment.amount) - U256::from(payment.refunded) {
            return refuse("the refund is more than what is left of the payment");
        }
        let nonce = self
            .contract(async {
                direct_pay.refundNonceFor(call.payment_id, call.amount.0, call.refunded_before.0).call().await
            })
            .await?;
        if nonce != call.nonce {
            return refuse("the note's nonce is not DirectPay's for this refund");
        }
        if self.balance(self.need(self.contracts.usdt, "USDT")?, call.merchant).await? < call.amount.0 {
            return refuse("the shop does not have enough USDT for the refund");
        }
        Ok(())
    }

    /// Whether the lock has timed out is the chain's call: the dry run runs on its clock.
    async fn check_refund(&self, lock_id: B256) -> Result<(), ChainError> {
        match self.source_lock_state(lock_id).await?.state {
            LOCK_OPEN => Ok(()),
            LOCK_CLAIMED => refuse("Bob claimed the lock; there is nothing to refund"),
            _ => refuse("there is no open lock with this id"),
        }
    }
}

impl Chain for Rpc {
    fn sender(&self) -> Address {
        self.sender
    }

    async fn done(&self, call: &Call) -> Result<bool, ChainError> {
        match call {
            Call::DirectPay(call) => self.note_used(self.need(self.contracts.usdt, "USDT")?, call.payer, call.nonce).await,
            Call::SourceLock(call) => self.note_used(call.token, call.payer, call.lock_id).await,
            Call::DestinationClaim(call) => Ok(self.destination_lock_state(call.lock_id).await?.state == LOCK_CLAIMED),
            Call::SourceRefund { lock_id } => Ok(self.source_lock_state(*lock_id).await?.state == LOCK_REFUNDED),
            Call::DirectRefund(call) => {
                self.note_used(self.need(self.contracts.usdt, "USDT")?, call.merchant, call.nonce).await
            }
        }
    }

    async fn check(&self, call: &Call, now: u64) -> Result<(), ChainError> {
        match call {
            Call::DirectPay(call) => self.check_pay(call).await?,
            Call::SourceLock(call) => self.check_lock(call, now).await?,
            Call::DestinationClaim(call) => self.check_claim(call, now).await?,
            Call::SourceRefund { lock_id } => self.check_refund(*lock_id).await?,
            Call::DirectRefund(call) => self.check_direct_refund(call).await?,
        }
        self.dry_run(call).await
    }

    async fn sign(&self, call: &Call, nonce: Option<u64>, fee_bumps: u32) -> Result<SignedTx, ChainError> {
        let (to, input) = self.target(call)?;
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
            .with_to(to)
            .with_input(input)
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
            // another one): either way the receipt and the chain decide what happened.
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

    async fn find_done(&self, call: &Call) -> Result<Option<B256>, ChainError> {
        let (contract, event) = match call {
            Call::DirectPay(_) => (self.need(self.contracts.direct_pay, "DirectPay")?, IDirectPay::Paid::SIGNATURE_HASH),
            Call::SourceLock(_) => (self.need(self.contracts.source_lock, "SourceLock")?, ISourceLock::Locked::SIGNATURE_HASH),
            Call::DestinationClaim(_) => {
                (self.need(self.contracts.destination_lock, "DestinationLock")?, IDestinationLock::Settled::SIGNATURE_HASH)
            }
            Call::SourceRefund { .. } => {
                (self.need(self.contracts.source_lock, "SourceLock")?, ISourceLock::Refunded::SIGNATURE_HASH)
            }
            Call::DirectRefund(_) => (self.need(self.contracts.direct_pay, "DirectPay")?, IDirectPay::Refunded::SIGNATURE_HASH),
        };
        let latest = self.rpc(self.provider.get_block_number().into_future()).await?;
        let filter = Filter::new()
            .address(contract)
            .event_signature(event)
            .topic1(call.log_topic())
            .from_block(latest.saturating_sub(LOG_LOOKBACK_BLOCKS))
            .to_block(latest);
        let logs = self.rpc(self.provider.get_logs(&filter).into_future()).await?;
        // A payment can be refunded in parts, all under its id: ours is the one that brought the
        // total to what this note was for.
        let ours = |log: &&alloy::rpc::types::Log| match call {
            Call::DirectRefund(call) => IDirectPay::Refunded::decode_log_data(log.data())
                .is_ok_and(|event| event.totalRefunded == call.refunded_before.0 + call.amount.0),
            _ => true,
        };
        Ok(logs.iter().find(ours).and_then(|log| log.transaction_hash))
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
