//! One pass of the Matchmaker: read new confirmed logs on every chain, report them, act on them,
//! then do the time-based duties (redeem settled source locks, refund expired locks).

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use alloy::{
    network::{EthereumWallet, TransactionBuilder},
    primitives::{Address, B256, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::{
        client::RpcClient,
        types::{BlockNumberOrTag, Filter, Log, TransactionRequest},
    },
    signers::local::PrivateKeySigner,
    transports::http::reqwest,
};
use bindings::{DestinationLock, IAqua, IFiatToken, PayoutPool, SourceLock};

use crate::{
    config::Config,
    events::{self, Action, Source},
    order_desk::{OrderDesk, OrderLookup, ReportError},
    rules::{self, SourceLockView},
    store::{Fill, Store},
};

/// How far back the first run starts, so a lock made just before start-up is not missed.
const FIRST_RUN_LOOKBACK: u64 = 200;
/// A log whose handling fails this many times is skipped and logged loudly.
const MAX_LOG_FAILURES: u64 = 30;
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(90);
/// Lock states as stored on chain.
const NONE: u8 = 0;
const OPEN: u8 = 1;
const CLAIMED: u8 = 2;

pub type Result<T> = std::result::Result<T, String>;

/// Error text with any URL removed, so RPC keys in URLs never reach the logs.
fn err(e: impl std::fmt::Display) -> String {
    redact_urls(&e.to_string())
}

fn redact_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("http://").or_else(|| rest.find("https://")) {
        out.push_str(&rest[..start]);
        out.push_str("<rpc>");
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || c == ')' || c == '"' || c == '\'')
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// What happened to a transaction the Matchmaker tried to send.
enum Sent {
    Mined(B256),
    /// The simulation reverted, so nothing was sent.
    WouldRevert(String),
}

struct Watched {
    chain_id: u64,
    provider: DynProvider,
    confirmations: u64,
    contracts: Vec<(Address, Source)>,
    /// Block range used for `eth_getLogs`; halved when a provider refuses a range.
    range: AtomicU64,
}

pub struct Worker {
    config: Config,
    store: Store,
    desk: OrderDesk,
    filler: Address,
    chains: Vec<Watched>,
    shop_chain_id: u64,
    usdt: Address,
    payout_pool: Address,
    destination_lock: Address,
    /// Our SourceLock and the chain's USDC on each customer chain.
    source_locks: HashMap<u64, Address>,
    usdc: HashMap<u64, Address>,
}

impl Worker {
    pub async fn new(config: Config) -> Result<Worker> {
        let signer: PrivateKeySigner = config
            .filler_key
            .parse()
            .map_err(|_| "FILLER_PRIVATE_KEY is not a key")?;
        let filler = signer.address();
        let wallet = EthereumWallet::from(signer);
        let store = Store::open(&config.db_path).await.map_err(err)?;
        let desk = OrderDesk::new(config.order_desk_url.clone(), config.internal_token.clone());

        let shop = config.chains.shop_chain();
        let usdt = shop
            .tokens
            .get("USDT")
            .ok_or("chains.json: shop chain has no USDT")?
            .address;
        let payout_pool = shop
            .contracts
            .payout_pool
            .ok_or("chains.json: no payoutPool")?;
        let destination_lock = shop
            .contracts
            .destination_lock
            .ok_or("chains.json: no destinationLock")?;
        let aqua = shop.contracts.aqua.ok_or("chains.json: no aqua")?;

        let http = reqwest::Client::builder()
            .timeout(RPC_TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(err)?;
        let connect = |chain_id: u64, rpc: &str| -> Result<DynProvider> {
            let url = config
                .rpc_for(chain_id, rpc)
                .parse()
                .map_err(|_| format!("bad RPC url for chain {chain_id}"))?;
            let client = RpcClient::new_http_with_client(http.clone(), url);
            // The nonce is read from the chain for every transaction, so a send that fails before
            // reaching the chain can never leave a gap that blocks later transactions.
            Ok(ProviderBuilder::new()
                .disable_recommended_fillers()
                .with_gas_estimation()
                .with_simple_nonce_management()
                .fetch_chain_id()
                .wallet(wallet.clone())
                .connect_client(client)
                .erased())
        };

        let mut chains = vec![];
        let mut shop_contracts = vec![
            (payout_pool, Source::PayoutPool),
            (destination_lock, Source::DestinationLock),
        ];
        if let Some(direct_pay) = shop.contracts.direct_pay {
            shop_contracts.push((direct_pay, Source::DirectPay));
        }
        chains.push(Watched {
            chain_id: shop.chain_id,
            provider: connect(shop.chain_id, &shop.rpc)?,
            confirmations: shop.confirmations,
            contracts: shop_contracts,
            range: AtomicU64::new(config.max_block_range),
        });

        let mut source_locks = HashMap::new();
        let mut usdc = HashMap::new();
        for chain in config.chains.customer_chains() {
            let Some(lock) = chain.contracts.source_lock else {
                continue;
            };
            let token = chain
                .tokens
                .get("USDC")
                .ok_or(format!("chains.json: chain {} has no USDC", chain.chain_id))?;
            source_locks.insert(chain.chain_id, lock);
            usdc.insert(chain.chain_id, token.address);
            chains.push(Watched {
                chain_id: chain.chain_id,
                provider: connect(chain.chain_id, &chain.rpc)?,
                confirmations: chain.confirmations,
                contracts: vec![(lock, Source::SourceLock)],
                range: AtomicU64::new(config.max_block_range),
            });
        }

        let worker = Worker {
            shop_chain_id: shop.chain_id,
            config,
            store,
            desk,
            filler,
            chains,
            usdt,
            payout_pool,
            destination_lock,
            source_locks,
            usdc,
        };
        worker.check_setup(aqua).await?;
        Ok(worker)
    }

    /// Refuses to start against the wrong contracts or a strategy Bob never shipped.
    async fn check_setup(&self, aqua: Address) -> Result<()> {
        let shop = self.provider(self.shop_chain_id)?;
        let pool = PayoutPool::new(self.payout_pool, shop);
        let wired = pool.destinationLock().call().await.map_err(err)?;
        if wired != self.destination_lock {
            return Err(format!(
                "chains.json destinationLock {} is not the pool's lock {wired}",
                self.destination_lock
            ));
        }
        let hash = pool
            .strategyHashOf(self.strategy()?)
            .call()
            .await
            .map_err(err)?;
        let shipped = IAqua::new(aqua, shop)
            .rawBalances(
                self.config.provider.address,
                self.payout_pool,
                hash,
                self.usdt,
            )
            .call()
            .await
            .map_err(err)?;
        if shipped.tokensCount == 0 || shipped.tokensCount == u8::MAX {
            return Err(format!(
                "Bob's strategy {hash} is not active on Aqua: check BOB_ADDRESS, STRATEGY_SALT, STRATEGY_MAX_PER_ORDER and STRATEGY_FEE_BPS"
            ));
        }
        eprintln!(
            "matchmaker: strategy {hash} active, allowance {}",
            shipped.balance
        );
        Ok(())
    }

    pub fn filler(&self) -> Address {
        self.filler
    }

    fn strategy(&self) -> Result<PayoutPool::Strategy> {
        Ok(PayoutPool::Strategy {
            maker: self.config.provider.address,
            token: self.usdt,
            filler: self.filler,
            maxPerOrder: u128::try_from(self.config.provider.max_per_order)
                .map_err(|_| "maxPerOrder too large")?,
            feeBps: self.config.provider.fee_bps,
            salt: self.config.strategy_salt,
        })
    }

    fn provider(&self, chain_id: u64) -> Result<&DynProvider> {
        self.chains
            .iter()
            .find(|c| c.chain_id == chain_id)
            .map(|c| &c.provider)
            .ok_or_else(|| format!("not watching chain {chain_id}"))
    }

    async fn chain_time(&self, chain_id: u64) -> Result<u64> {
        let block = self
            .provider(chain_id)?
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .map_err(err)?
            .ok_or("no latest block")?;
        Ok(block.header.timestamp)
    }

    /// Simulates, then sends and waits (with a time limit) for the receipt. Only a revert in the
    /// simulation counts as `WouldRevert`; network errors are returned as `Err` to be retried.
    async fn send(&self, chain_id: u64, tx: TransactionRequest) -> Result<Sent> {
        let provider = self.provider(chain_id)?;
        let tx = tx.with_from(self.filler);
        if let Err(e) = provider.call(tx.clone()).await {
            let reverted = e
                .as_error_resp()
                .is_some_and(|r| r.as_revert_data().is_some() || r.message.contains("revert"));
            return if reverted {
                Ok(Sent::WouldRevert(err(e)))
            } else {
                Err(err(e))
            };
        }
        let pending = provider.send_transaction(tx).await.map_err(err)?;
        let receipt = pending
            .with_timeout(Some(RECEIPT_TIMEOUT))
            .get_receipt()
            .await
            .map_err(err)?;
        if receipt.status() {
            Ok(Sent::Mined(receipt.transaction_hash))
        } else {
            Err(format!(
                "transaction {} reverted on chain",
                receipt.transaction_hash
            ))
        }
    }

    /// One full pass. A failure on one chain or one lock never stops the others.
    pub async fn tick(&self) {
        for i in 0..self.chains.len() {
            if let Err(e) = self.scan(i).await {
                eprintln!("matchmaker: chain {}: {e}", self.chains[i].chain_id);
            }
            // Redeeming is the most time-critical duty, so it runs after every chain's scan.
            self.redeem_settled().await;
        }
        self.flush_outbox().await;
        self.refund_expired_destinations().await;
        self.refund_expired_sources().await;
    }

    /// Reads the next confirmed block range on one chain. The cursor only moves past a range once
    /// every log in it was handled and the node is known to have the range's last block.
    async fn scan(&self, index: usize) -> Result<()> {
        let watched = &self.chains[index];
        let head = watched.provider.get_block_number().await.map_err(err)?;
        let Some(safe) = head.checked_sub(watched.confirmations) else {
            return Ok(());
        };
        let from = match self.store.cursor(watched.chain_id).await.map_err(err)? {
            Some(next) => next,
            None => safe.saturating_sub(FIRST_RUN_LOOKBACK),
        };
        if from > safe {
            return Ok(());
        }
        let range = watched.range.load(Ordering::Relaxed).max(1);
        let to = safe.min(from.saturating_add(range - 1));
        // A load-balanced RPC can answer from a node that is behind; only trust logs up to a block
        // this node actually has.
        if watched
            .provider
            .get_block_by_number(to.into())
            .await
            .map_err(err)?
            .is_none()
        {
            return Ok(());
        }
        let addresses: Vec<Address> = watched.contracts.iter().map(|(a, _)| *a).collect();
        let filter = Filter::new()
            .address(addresses)
            .from_block(from)
            .to_block(to);
        let mut logs = match watched.provider.get_logs(&filter).await {
            Ok(logs) => logs,
            Err(e) => {
                if range > 1 {
                    watched.range.store(range / 2, Ordering::Relaxed);
                }
                return Err(format!("get_logs {from}..={to}: {}", err(e)));
            }
        };
        logs.sort_by_key(|l| (l.block_number, l.log_index));

        for log in &logs {
            let Some(source) = watched
                .contracts
                .iter()
                .find(|(a, _)| *a == log.address())
                .map(|(_, s)| *s)
            else {
                continue;
            };
            if let Err(e) = self.handle(watched.chain_id, source, log).await {
                let (Some(tx), Some(index)) = (log.transaction_hash, log.log_index) else {
                    return Err(e);
                };
                let count = self
                    .store
                    .bump_failure(watched.chain_id, tx, index)
                    .await
                    .map_err(err)?;
                if count < MAX_LOG_FAILURES {
                    return Err(format!("log {tx}:{index} (try {count}): {e}"));
                }
                eprintln!("matchmaker: GIVING UP on log {tx}:{index} after {count} tries: {e}");
            }
        }
        self.store
            .set_cursor(watched.chain_id, to + 1)
            .await
            .map_err(err)
    }

    async fn handle(&self, chain_id: u64, source: Source, log: &Log) -> Result<()> {
        let (report, action) = events::decode(chain_id, source, log);
        if let Some(event) = report {
            let body = serde_json::to_string(&event).map_err(err)?;
            self.store
                .enqueue(chain_id, event.tx_hash, event.log_index, &body)
                .await
                .map_err(err)?;
        }
        match action {
            Action::Locked(locked) => self.on_locked(chain_id, locked).await,
            Action::Settled(settled) => self.on_settled(settled).await,
            Action::None => Ok(()),
        }
    }

    /// A customer locked on a source chain. If Bob already filled it (an earlier run stopped before
    /// recording), record it from the chain. Otherwise, if the lock is for Bob and passes every
    /// rule, open the destination lock for the shop with Bob's USDT.
    async fn on_locked(&self, chain_id: u64, locked: SourceLock::Locked) -> Result<()> {
        let bob = self.config.provider.address;
        if locked.receiver != bob {
            return Ok(());
        }
        let Some(&lock_address) = self.source_locks.get(&chain_id) else {
            return Ok(());
        };
        self.store
            .watch_source_lock(chain_id, locked.lockId, locked.expiry)
            .await
            .map_err(err)?;
        let source = SourceLock::new(lock_address, self.provider(chain_id)?);
        let on_chain = source.locks(locked.lockId).call().await.map_err(err)?;
        let view = SourceLockView {
            open: on_chain.state == OPEN,
            receiver: on_chain.receiver,
            amount: U256::from(on_chain.amount),
            hashlock: on_chain.hashlock,
            expiry: on_chain.expiry,
        };

        let shop = self.provider(self.shop_chain_id)?;
        let destination = DestinationLock::new(self.destination_lock, shop);
        let destination_lock_id = destination
            .lockIdFor(U256::from(chain_id), locked.lockId, bob)
            .call()
            .await
            .map_err(err)?;
        let existing = destination
            .locks(destination_lock_id)
            .call()
            .await
            .map_err(err)?;
        if existing.state != NONE {
            if existing.maker != bob || existing.hashlock != view.hashlock {
                return Err(format!(
                    "destination lock {destination_lock_id} does not match source lock {}",
                    locked.lockId
                ));
            }
            let status = match existing.state {
                OPEN | CLAIMED => "filled", // CLAIMED: its Settled log moves it to settled
                _ => "refunded",
            };
            let new = self
                .store
                .record_fill_if_missing(&Fill {
                    order_id: on_chain.orderId,
                    src_chain_id: chain_id,
                    src_lock_id: locked.lockId,
                    src_expiry: view.expiry,
                    destination_lock_id,
                    destination_expiry: existing.expiry,
                    amount: U256::from(existing.amount),
                    status: status.into(),
                    secret: None,
                })
                .await
                .map_err(err)?;
            if new {
                eprintln!(
                    "matchmaker: recorded an earlier fill for order {} from the chain",
                    on_chain.orderId
                );
            }
            return Ok(());
        }

        let usdc = self.usdc[&chain_id];
        if IFiatToken::new(usdc, self.provider(chain_id)?)
            .isBlacklisted(bob)
            .call()
            .await
            .map_err(err)?
        {
            eprintln!(
                "matchmaker: skip lock {}: Circle has blacklisted Bob, he could not redeem",
                locked.lockId
            );
            return Ok(());
        }
        let gas = self
            .provider(chain_id)?
            .get_balance(self.filler)
            .await
            .map_err(err)?;
        if gas < U256::from(self.config.min_filler_gas_wei) {
            return Err(format!("filler has too little gas on chain {chain_id} to redeem later ({gas} wei); top it up"));
        }

        let order = match self.desk.order(on_chain.orderId).await? {
            OrderLookup::Found(order) => order,
            OrderLookup::NotFound => {
                eprintln!(
                    "matchmaker: skip lock {}: order {} is unknown",
                    locked.lockId, on_chain.orderId
                );
                return Ok(());
            }
        };
        let already = self.store.has_fill(on_chain.orderId).await.map_err(err)?;
        let src_now = self.chain_time(chain_id).await?;
        let dest_now = self.chain_time(self.shop_chain_id).await?;
        let plan = match rules::fill_decision(
            &view,
            &order,
            &self.config.provider,
            already,
            src_now,
            dest_now,
            self.config.timing,
        ) {
            Ok(plan) => plan,
            Err(skip) => {
                eprintln!("matchmaker: skip lock {}: {skip:?}", locked.lockId);
                return Ok(());
            }
        };

        let fill = PayoutPool::Fill {
            orderId: on_chain.orderId,
            merchant: plan.merchant,
            amount: plan.amount,
            hashlock: view.hashlock,
            expiry: plan.destination_expiry,
            srcChainId: U256::from(chain_id),
            srcLockId: locked.lockId,
        };
        let tx = PayoutPool::new(self.payout_pool, shop)
            .fill(self.strategy()?, fill)
            .into_transaction_request();
        match self.send(self.shop_chain_id, tx).await? {
            Sent::WouldRevert(reason) => {
                eprintln!(
                    "matchmaker: skip lock {}: fill would revert: {reason}",
                    locked.lockId
                );
                Ok(())
            }
            Sent::Mined(hash) => {
                eprintln!("matchmaker: filled order {} in {hash}", on_chain.orderId);
                self.store
                    .record_fill_if_missing(&Fill {
                        order_id: on_chain.orderId,
                        src_chain_id: chain_id,
                        src_lock_id: locked.lockId,
                        src_expiry: view.expiry,
                        destination_lock_id,
                        destination_expiry: plan.destination_expiry,
                        amount: plan.amount,
                        status: "filled".into(),
                        secret: None,
                    })
                    .await
                    .map_err(err)?;
                Ok(())
            }
        }
    }

    /// The shop was paid on Whitechain and the secret is public. The log came from our
    /// DestinationLock. If the fill was never recorded (a run stopped right after filling), it is
    /// recorded now, once the chain confirms the lock is Bob's and matches the source lock.
    async fn on_settled(&self, settled: DestinationLock::Settled) -> Result<()> {
        if self
            .store
            .fill_by_destination(settled.lockId)
            .await
            .map_err(err)?
            .is_some()
        {
            return self
                .store
                .mark_settled(settled.lockId, settled.secret)
                .await
                .map_err(err);
        }
        let bob = self.config.provider.address;
        let Ok(src_chain_id) = u64::try_from(settled.srcChainId) else {
            return Ok(());
        };
        let Some(&lock_address) = self.source_locks.get(&src_chain_id) else {
            return Ok(());
        };
        let shop = self.provider(self.shop_chain_id)?;
        let destination = DestinationLock::new(self.destination_lock, shop);
        let expected = destination
            .lockIdFor(settled.srcChainId, settled.srcLockId, bob)
            .call()
            .await
            .map_err(err)?;
        if expected != settled.lockId {
            return Ok(()); // someone else's destination lock
        }
        let source = SourceLock::new(lock_address, self.provider(src_chain_id)?)
            .locks(settled.srcLockId)
            .call()
            .await
            .map_err(err)?;
        if source.receiver != bob {
            return Ok(());
        }
        let existing = destination
            .locks(settled.lockId)
            .call()
            .await
            .map_err(err)?;
        self.store
            .record_fill_if_missing(&Fill {
                order_id: settled.orderId,
                src_chain_id,
                src_lock_id: settled.srcLockId,
                src_expiry: source.expiry,
                destination_lock_id: settled.lockId,
                destination_expiry: existing.expiry,
                amount: settled.amount,
                status: "settled".into(),
                secret: Some(settled.secret),
            })
            .await
            .map_err(err)?;
        eprintln!(
            "matchmaker: recorded an unrecorded fill for order {} from its Settled log",
            settled.orderId
        );
        Ok(())
    }

    /// Source redeem for every settled fill: Bob collects the customer's USDC with the secret.
    async fn redeem_settled(&self) {
        let fills = match self.store.fills_with_status("settled").await {
            Ok(fills) => fills,
            Err(e) => return eprintln!("matchmaker: source redeem: {}", err(e)),
        };
        for fill in fills {
            if let Err(e) = self.redeem(&fill).await {
                eprintln!("matchmaker: source redeem for order {}: {e}", fill.order_id);
            }
        }
    }

    async fn redeem(&self, fill: &Fill) -> Result<()> {
        let Some(secret) = fill.secret else {
            return Ok(());
        };
        let Some(&lock_address) = self.source_locks.get(&fill.src_chain_id) else {
            return Ok(());
        };
        let lock = SourceLock::new(lock_address, self.provider(fill.src_chain_id)?);
        let on_chain = lock.locks(fill.src_lock_id).call().await.map_err(err)?;
        if on_chain.state == CLAIMED {
            return self
                .store
                .set_fill_status(fill.order_id, "claimed")
                .await
                .map_err(err);
        }
        let now = self.chain_time(fill.src_chain_id).await?;
        if on_chain.state != OPEN || now >= on_chain.expiry {
            eprintln!(
                "matchmaker: MISSED source lock {} for order {}: the shop was paid but the lock expired or was refunded",
                fill.src_lock_id, fill.order_id
            );
            return self
                .store
                .set_fill_status(fill.order_id, "missed")
                .await
                .map_err(err);
        }
        match self
            .send(
                fill.src_chain_id,
                lock.claim(fill.src_lock_id, secret)
                    .into_transaction_request(),
            )
            .await?
        {
            Sent::Mined(hash) => {
                eprintln!(
                    "matchmaker: redeemed source lock {} in {hash}",
                    fill.src_lock_id
                );
                self.store
                    .set_fill_status(fill.order_id, "claimed")
                    .await
                    .map_err(err)
            }
            Sent::WouldRevert(reason) => Err(format!("claim would revert: {reason}")),
        }
    }

    /// Bob's destination locks that timed out unclaimed: send the USDT back into his strategy.
    async fn refund_expired_destinations(&self) {
        let fills = match self.store.fills_with_status("filled").await {
            Ok(fills) => fills,
            Err(e) => return eprintln!("matchmaker: destination refunds: {}", err(e)),
        };
        if fills.is_empty() {
            return;
        }
        let now = match self.chain_time(self.shop_chain_id).await {
            Ok(now) => now,
            Err(e) => return eprintln!("matchmaker: destination refunds: {e}"),
        };
        for fill in fills.into_iter().filter(|f| f.destination_expiry <= now) {
            if let Err(e) = self.refund_destination(&fill).await {
                eprintln!(
                    "matchmaker: destination refund for order {}: {e}",
                    fill.order_id
                );
            }
        }
    }

    async fn refund_destination(&self, fill: &Fill) -> Result<()> {
        let destination =
            DestinationLock::new(self.destination_lock, self.provider(self.shop_chain_id)?);
        let state = destination
            .locks(fill.destination_lock_id)
            .call()
            .await
            .map_err(err)?
            .state;
        match state {
            OPEN => match self
                .send(
                    self.shop_chain_id,
                    destination
                        .refund(fill.destination_lock_id)
                        .into_transaction_request(),
                )
                .await?
            {
                Sent::Mined(hash) => {
                    eprintln!(
                        "matchmaker: refunded destination lock {} in {hash}",
                        fill.destination_lock_id
                    );
                    self.store
                        .set_fill_status(fill.order_id, "refunded")
                        .await
                        .map_err(err)
                }
                Sent::WouldRevert(reason) => Err(format!("refund would revert: {reason}")),
            },
            CLAIMED => Ok(()), // its Settled log moves it to settled
            _ => self
                .store
                .set_fill_status(fill.order_id, "refunded")
                .await
                .map_err(err),
        }
    }

    /// Customer locks for Bob that timed out: send the USDC back to the customer.
    async fn refund_expired_sources(&self) {
        for (&chain_id, &lock_address) in &self.source_locks {
            let now = match self.chain_time(chain_id).await {
                Ok(now) => now,
                Err(e) => {
                    eprintln!("matchmaker: source refunds on chain {chain_id}: {e}");
                    continue;
                }
            };
            let expired = match self.store.expired_source_locks(now).await {
                Ok(expired) => expired,
                Err(e) => return eprintln!("matchmaker: source refunds: {}", err(e)),
            };
            for (lock_chain, lock_id) in expired.into_iter().filter(|(c, _)| *c == chain_id) {
                if let Err(e) = self
                    .refund_source(lock_chain, lock_address, lock_id, now)
                    .await
                {
                    eprintln!("matchmaker: source refund for lock {lock_id}: {e}");
                }
            }
        }
    }

    async fn refund_source(
        &self,
        chain_id: u64,
        lock_address: Address,
        lock_id: B256,
        now: u64,
    ) -> Result<()> {
        let lock = SourceLock::new(lock_address, self.provider(chain_id)?);
        let on_chain = lock.locks(lock_id).call().await.map_err(err)?;
        if on_chain.state != OPEN {
            return self
                .store
                .source_lock_done(chain_id, lock_id)
                .await
                .map_err(err);
        }
        if on_chain.expiry > now {
            return Ok(());
        }
        match self
            .send(chain_id, lock.refund(lock_id).into_transaction_request())
            .await?
        {
            Sent::Mined(hash) => {
                eprintln!("matchmaker: refunded source lock {lock_id} in {hash}");
                self.store
                    .source_lock_done(chain_id, lock_id)
                    .await
                    .map_err(err)
            }
            Sent::WouldRevert(reason) => Err(format!("refund would revert: {reason}")),
        }
    }

    /// Reports queued events in order. A temporary failure stops here to keep the order; an event
    /// the Order Desk refuses for good is dropped and logged, so it cannot block the rest.
    async fn flush_outbox(&self) {
        let items = match self.store.outbox().await {
            Ok(items) => items,
            Err(e) => return eprintln!("matchmaker: reporting: {}", err(e)),
        };
        for (chain_id, tx_hash, log_index, body) in items {
            match self.desk.report(&body).await {
                Ok(()) => {}
                Err(ReportError::Rejected(reason)) => {
                    eprintln!("matchmaker: Order Desk rejected event {tx_hash}:{log_index} for good, dropping it: {reason}");
                }
                Err(ReportError::Retry(reason)) => {
                    return eprintln!("matchmaker: reporting: {reason}")
                }
            }
            if let Err(e) = self.store.dequeue(chain_id, &tx_hash, log_index).await {
                return eprintln!("matchmaker: reporting: {}", err(e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::redact_urls;

    #[test]
    fn urls_are_removed_from_error_text() {
        assert_eq!(
            redact_urls("error sending request for url (https://rpc.ankr.com/base_sepolia/SECRETKEY): timeout"),
            "error sending request for url (<rpc>): timeout"
        );
        assert_eq!(redact_urls("no url here"), "no url here");
        assert_eq!(redact_urls("http://a/1 and https://b/2"), "<rpc> and <rpc>");
    }
}
