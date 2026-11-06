//! One pass of the Matchmaker: read new confirmed logs on every chain, report them, act on them,
//! then do the time-based duties (redeem settled source locks, refund expired locks).

use std::collections::HashMap;

use alloy::{
    network::EthereumWallet,
    primitives::{Address, B256, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::{BlockNumberOrTag, Filter, Log},
    signers::local::PrivateKeySigner,
};
use bindings::{DestinationLock, PayoutPool, SourceLock};

use crate::{
    config::Config,
    events::{self, Action, Source},
    order_desk::{OrderDesk, OrderLookup},
    rules::{self, SourceLockView},
    store::{Fill, Store},
};

/// How far back the first run starts, so a lock made just before start-up is not missed.
const FIRST_RUN_LOOKBACK: u64 = 200;
/// Lock states as stored on chain.
const OPEN: u8 = 1;
const CLAIMED: u8 = 2;

pub type Result<T> = std::result::Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

struct Watched {
    chain_id: u64,
    provider: DynProvider,
    confirmations: u64,
    contracts: Vec<(Address, Source)>,
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
    /// Our SourceLock on each customer chain.
    source_locks: HashMap<u64, Address>,
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

        let connect = |chain_id: u64, rpc: &str| -> Result<DynProvider> {
            let url = config
                .rpc_for(chain_id, rpc)
                .parse()
                .map_err(|_| format!("bad RPC url for chain {chain_id}"))?;
            Ok(ProviderBuilder::new()
                .wallet(wallet.clone())
                .connect_http(url)
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
        });

        let mut source_locks = HashMap::new();
        for chain in config.chains.customer_chains() {
            let Some(lock) = chain.contracts.source_lock else {
                continue;
            };
            source_locks.insert(chain.chain_id, lock);
            chains.push(Watched {
                chain_id: chain.chain_id,
                provider: connect(chain.chain_id, &chain.rpc)?,
                confirmations: chain.confirmations,
                contracts: vec![(lock, Source::SourceLock)],
            });
        }

        Ok(Worker {
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
        })
    }

    pub fn filler(&self) -> Address {
        self.filler
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

    /// One full pass. Errors on one chain don't stop the others.
    pub async fn tick(&self) {
        for i in 0..self.chains.len() {
            if let Err(e) = self.scan(i).await {
                eprintln!("matchmaker: chain {}: {e}", self.chains[i].chain_id);
            }
        }
        if let Err(e) = self.flush_outbox().await {
            eprintln!("matchmaker: reporting: {e}");
        }
        if let Err(e) = self.redeem_settled().await {
            eprintln!("matchmaker: source redeem: {e}");
        }
        if let Err(e) = self.refund_expired_destinations().await {
            eprintln!("matchmaker: destination refunds: {e}");
        }
        if let Err(e) = self.refund_expired_sources().await {
            eprintln!("matchmaker: source refunds: {e}");
        }
    }

    /// Reads the next confirmed block range on one chain. The cursor only moves past a range once
    /// every log in it was handled, so a failure means the range is read again next time.
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
        let to = safe.min(from + self.config.max_block_range - 1);
        let addresses: Vec<Address> = watched.contracts.iter().map(|(a, _)| *a).collect();
        let filter = Filter::new()
            .address(addresses)
            .from_block(from)
            .to_block(to);
        let mut logs = watched.provider.get_logs(&filter).await.map_err(err)?;
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
            self.handle(watched.chain_id, source, log).await?;
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

    /// A customer locked on a source chain. If the lock is for Bob and passes every rule, open the
    /// destination lock for the shop with Bob's USDT.
    async fn on_locked(&self, chain_id: u64, locked: SourceLock::Locked) -> Result<()> {
        if locked.receiver != self.config.provider.address {
            return Ok(());
        }
        self.store
            .watch_source_lock(chain_id, locked.lockId, locked.expiry)
            .await
            .map_err(err)?;
        let lock_address = self.source_locks[&chain_id];
        let on_chain = SourceLock::new(lock_address, self.provider(chain_id)?)
            .locks(locked.lockId)
            .call()
            .await
            .map_err(err)?;
        let view = SourceLockView {
            open: on_chain.state == OPEN,
            receiver: on_chain.receiver,
            amount: U256::from(on_chain.amount),
            hashlock: on_chain.hashlock,
            expiry: on_chain.expiry,
        };

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
        let now = self.chain_time(chain_id).await?;
        let plan = match rules::fill_decision(
            &view,
            &order,
            &self.config.provider,
            already,
            now,
            self.config.timing,
        ) {
            Ok(plan) => plan,
            Err(skip) => {
                eprintln!("matchmaker: skip lock {}: {skip:?}", locked.lockId);
                return Ok(());
            }
        };

        let shop = self.provider(self.shop_chain_id)?;
        let pool = PayoutPool::new(self.payout_pool, shop);
        let strategy = PayoutPool::Strategy {
            maker: self.config.provider.address,
            token: self.usdt,
            filler: self.filler,
            maxPerOrder: u128::try_from(self.config.provider.max_per_order)
                .map_err(|_| "maxPerOrder too large")?,
            feeBps: self.config.provider.fee_bps,
            salt: self.config.strategy_salt,
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
        let destination = DestinationLock::new(self.destination_lock, shop);
        let destination_lock_id = destination
            .lockIdFor(
                U256::from(chain_id),
                locked.lockId,
                self.config.provider.address,
            )
            .call()
            .await
            .map_err(err)?;

        if let Err(e) = pool.fill(strategy.clone(), fill.clone()).call().await {
            // Already filled by an earlier run that stopped before recording it: record it now.
            let existing = destination
                .locks(destination_lock_id)
                .call()
                .await
                .map_err(err)?;
            if existing.state != 0 {
                return self
                    .record_fill(
                        chain_id,
                        &locked,
                        &view,
                        destination_lock_id,
                        existing.expiry,
                        plan.amount,
                    )
                    .await;
            }
            eprintln!(
                "matchmaker: skip lock {}: fill would revert: {e}",
                locked.lockId
            );
            return Ok(());
        }
        let receipt = pool
            .fill(strategy, fill)
            .send()
            .await
            .map_err(err)?
            .get_receipt()
            .await
            .map_err(err)?;
        if !receipt.status() {
            return Err(format!(
                "fill for lock {} reverted in {}",
                locked.lockId, receipt.transaction_hash
            ));
        }
        eprintln!(
            "matchmaker: filled order {} in {}",
            on_chain.orderId, receipt.transaction_hash
        );
        self.record_fill(
            chain_id,
            &locked,
            &view,
            destination_lock_id,
            plan.destination_expiry,
            plan.amount,
        )
        .await
    }

    async fn record_fill(
        &self,
        chain_id: u64,
        locked: &SourceLock::Locked,
        view: &SourceLockView,
        destination_lock_id: B256,
        destination_expiry: u64,
        amount: U256,
    ) -> Result<()> {
        self.store
            .insert_fill(&Fill {
                order_id: locked.orderId,
                src_chain_id: chain_id,
                src_lock_id: locked.lockId,
                src_expiry: view.expiry,
                destination_lock_id,
                destination_expiry,
                amount,
                status: "filled".into(),
                secret: None,
            })
            .await
            .map_err(err)
    }

    /// The shop was paid on Whitechain and the secret is public. Only our own fills count: the log
    /// came from our DestinationLock, and the lock id must be one we recorded.
    async fn on_settled(&self, settled: DestinationLock::Settled) -> Result<()> {
        if self
            .store
            .fill_by_destination(settled.lockId)
            .await
            .map_err(err)?
            .is_some()
        {
            self.store
                .mark_settled(settled.lockId, settled.secret)
                .await
                .map_err(err)?;
        }
        Ok(())
    }

    /// Source redeem for every settled fill: Bob collects the customer's USDC with the secret.
    async fn redeem_settled(&self) -> Result<()> {
        for fill in self.store.fills_with_status("settled").await.map_err(err)? {
            let Some(secret) = fill.secret else { continue };
            let Some(&lock_address) = self.source_locks.get(&fill.src_chain_id) else {
                continue;
            };
            let lock = SourceLock::new(lock_address, self.provider(fill.src_chain_id)?);
            let state = lock
                .locks(fill.src_lock_id)
                .call()
                .await
                .map_err(err)?
                .state;
            if state != OPEN {
                let status = if state == CLAIMED { "claimed" } else { "lost" };
                eprintln!(
                    "matchmaker: source lock {} is no longer open; marking {status}",
                    fill.src_lock_id
                );
                self.store
                    .set_fill_status(fill.order_id, status)
                    .await
                    .map_err(err)?;
                continue;
            }
            let receipt = lock
                .claim(fill.src_lock_id, secret)
                .send()
                .await
                .map_err(err)?
                .get_receipt()
                .await
                .map_err(err)?;
            if receipt.status() {
                eprintln!(
                    "matchmaker: redeemed source lock {} in {}",
                    fill.src_lock_id, receipt.transaction_hash
                );
                self.store
                    .set_fill_status(fill.order_id, "claimed")
                    .await
                    .map_err(err)?;
            }
        }
        Ok(())
    }

    /// Bob's destination locks that timed out unclaimed: send the USDT back into his strategy.
    async fn refund_expired_destinations(&self) -> Result<()> {
        let fills = self.store.fills_with_status("filled").await.map_err(err)?;
        if fills.is_empty() {
            return Ok(());
        }
        let now = self.chain_time(self.shop_chain_id).await?;
        let destination =
            DestinationLock::new(self.destination_lock, self.provider(self.shop_chain_id)?);
        for fill in fills.into_iter().filter(|f| f.destination_expiry <= now) {
            let state = destination
                .locks(fill.destination_lock_id)
                .call()
                .await
                .map_err(err)?
                .state;
            if state == OPEN {
                let receipt = destination
                    .refund(fill.destination_lock_id)
                    .send()
                    .await
                    .map_err(err)?
                    .get_receipt()
                    .await
                    .map_err(err)?;
                if !receipt.status() {
                    continue;
                }
                eprintln!(
                    "matchmaker: refunded destination lock {} in {}",
                    fill.destination_lock_id, receipt.transaction_hash
                );
                self.store
                    .set_fill_status(fill.order_id, "refunded")
                    .await
                    .map_err(err)?;
            } else if state != CLAIMED {
                self.store
                    .set_fill_status(fill.order_id, "refunded")
                    .await
                    .map_err(err)?;
            }
            // CLAIMED: the Settled log will arrive and move it to settled.
        }
        Ok(())
    }

    /// Customer locks for Bob that timed out: send the USDC back to the customer.
    async fn refund_expired_sources(&self) -> Result<()> {
        let mut now_by_chain = HashMap::new();
        for chain_id in self.source_locks.keys() {
            now_by_chain.insert(*chain_id, self.chain_time(*chain_id).await?);
        }
        let latest = now_by_chain.values().copied().max().unwrap_or(0);
        for (chain_id, lock_id) in self.store.expired_source_locks(latest).await.map_err(err)? {
            let Some(&lock_address) = self.source_locks.get(&chain_id) else {
                continue;
            };
            let lock = SourceLock::new(lock_address, self.provider(chain_id)?);
            let on_chain = lock.locks(lock_id).call().await.map_err(err)?;
            if on_chain.state != OPEN {
                self.store
                    .source_lock_done(chain_id, lock_id)
                    .await
                    .map_err(err)?;
                continue;
            }
            if on_chain.expiry > now_by_chain[&chain_id] {
                continue;
            }
            let receipt = lock
                .refund(lock_id)
                .send()
                .await
                .map_err(err)?
                .get_receipt()
                .await
                .map_err(err)?;
            if receipt.status() {
                eprintln!(
                    "matchmaker: refunded source lock {lock_id} in {}",
                    receipt.transaction_hash
                );
                self.store
                    .source_lock_done(chain_id, lock_id)
                    .await
                    .map_err(err)?;
            }
        }
        Ok(())
    }

    /// Reports queued events in order. Stops at the first failure so order is kept.
    async fn flush_outbox(&self) -> Result<()> {
        for (chain_id, tx_hash, log_index, body) in self.store.outbox().await.map_err(err)? {
            self.desk.report(&body).await?;
            self.store
                .dequeue(chain_id, &tx_hash, log_index)
                .await
                .map_err(err)?;
        }
        Ok(())
    }
}
