//! The Matchmaker's duties. `tick` reads new confirmed logs on every chain, reports and acts on
//! them, retries logs that failed, reports events and refunds expired locks. `redeem_duty` runs on
//! its own loop so nothing else can delay Bob's source redeems.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use alloy::{
    network::{EthereumWallet, TransactionBuilder},
    primitives::{Address, B256, U256, keccak256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::{
        client::RpcClient,
        types::{BlockNumberOrTag, Filter, Log, TransactionRequest},
    },
    signers::local::PrivateKeySigner,
    sol_types::{SolEvent, SolValue},
    transports::http::reqwest,
};
use bindings::{DestinationLock, IAqua, IERC20, IFiatToken, PayoutPool, SourceLock};
use common::chains::Role;
use tokio::sync::Mutex;

use crate::{
    config::Config,
    events::{self, Action, Source},
    order_desk::{Order, OrderDesk, OrderLeg, OrderLookup, ReportError},
    price::{self, CoinPrice},
    rules::{self, LockedToken, SourceLockView},
    store::{Fill, FillLeg, PendingLog, SourceLeg, Store},
};

/// How far back the first run starts, so locks made before start-up are still seen (on Base
/// Sepolia under an hour; on Ethereum Sepolia about 20 hours).
const FIRST_RUN_LOOKBACK: u64 = 6_000;
/// A parked log whose handling fails permanently this many times is dropped and logged loudly.
/// Network and Order Desk outages are not counted: those logs are retried until they clear.
const MAX_LOG_FAILURES: u64 = 30;
/// Marks an error that retrying cannot fix (data on chain that does not match), as opposed to an
/// outage.
const PERMANENT: &str = "permanent: ";
/// How far back to search for a destination lock's Settled log when its secret is needed.
const SECRET_SEARCH_BLOCKS: u64 = 200_000;
/// Refunds sent per chain per pass, so a flood of expired locks cannot hold up other duties.
const REFUNDS_PER_PASS: usize = 5;
/// Log ranges read per chain per pass while catching up, so one chain's backlog cannot hold up
/// the others for long.
const MAX_RANGES_PER_PASS: usize = 20;
/// A node whose latest block is older than this is treated as behind, and its time not trusted.
const MAX_CLOCK_LAG: u64 = 120;
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(90);
/// DestinationLock's lock id tag (see DestinationLock.sol).
const DESTINATION_LOCK_TAG: &str = "uniform.destinationlock.lock.v1";
/// Lock states as stored on chain.
const NONE: u8 = 0;
const OPEN: u8 = 1;
const CLAIMED: u8 = 2;
const REFUNDED: u8 = 3;

pub type Result<T> = std::result::Result<T, String>;

/// Error text with any URL removed, so RPC keys in URLs never reach the logs.
fn err(e: impl std::fmt::Display) -> String {
    redact_urls(&e.to_string())
}

fn redact_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = [rest.find("http://"), rest.find("https://")]
        .into_iter()
        .flatten()
        .min()
    {
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

/// Whether an `eth_getLogs` error means "ask for fewer blocks" (as opposed to an outage or a rate
/// limit, which a smaller range would not fix).
fn is_range_error(message: &str) -> bool {
    let m = message.to_lowercase();
    if m.contains("rate") || m.contains("429") || m.contains("timed out") || m.contains("timeout") {
        return false;
    }
    [
        "block range",
        "range is too",
        "range too",
        "too many",
        "exceed",
        "response size",
        "limit",
    ]
    .iter()
    .any(|p| m.contains(p))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// DestinationLock.lockIdFor, computed locally:
/// keccak256(abi.encode(LOCK_TAG, chainid, lock, srcChainId, srcLockId, maker)).
pub fn destination_lock_id(
    chain_id: u64,
    lock: Address,
    src_chain_id: u64,
    src_lock_id: B256,
    maker: Address,
) -> B256 {
    let tag = keccak256(DESTINATION_LOCK_TAG.as_bytes());
    keccak256(
        (
            tag,
            U256::from(chain_id),
            lock,
            U256::from(src_chain_id),
            src_lock_id,
            maker,
        )
            .abi_encode(),
    )
}

/// The source locks that pay for an order, first one first (its destination lock names it), if
/// `seen` is one of them. An order that lists no signed locks yet is paid by none: anyone can lock
/// with an order's id, and a stranger's lock must not take the order's one fill.
fn legs_for(order: &Order, seen: OrderLeg) -> Option<Vec<OrderLeg>> {
    let same = |l: &OrderLeg| l.chain_id == seen.chain_id && l.lock_id == seen.lock_id;
    if !order.legs.iter().any(same) {
        return None;
    }
    let mut legs = order.legs.clone();
    legs.sort();
    legs.dedup_by_key(|l| (l.chain_id, l.lock_id));
    Some(legs)
}

fn source_legs(legs: &[OrderLeg]) -> Vec<SourceLeg> {
    legs.iter()
        .map(|l| SourceLeg {
            chain_id: l.chain_id,
            lock_id: l.lock_id,
            expiry: l.expiry,
        })
        .collect()
}

/// One source lock of an order as read from its chain, ready for the fill rules.
struct LegRead {
    leg: OrderLeg,
    kind: LockedToken,
    token: Address,
    /// CoinGecko id, for a `LockedToken::Priced` lock.
    price_id: Option<String>,
    view: SourceLockView,
    src_now: u64,
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
    /// Read up to the "safe" block instead of `confirmations` (rollup customer chains only).
    use_safe_tag: bool,
    contracts: Vec<(Address, Source)>,
    /// Block range used for `eth_getLogs`: halved when a provider refuses a range, grown back after.
    range: AtomicU64,
    /// The largest range the provider has not refused; growth never goes past it.
    ceiling: AtomicU64,
    /// One transaction at a time per chain, so the two loops never race for a nonce.
    send_lock: Mutex<()>,
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
    aqua: Address,
    /// Bob's strategy on Aqua, checked at start-up.
    strategy_hash: B256,
    /// Our SourceLock and the tokens it accepts on each customer chain.
    source_locks: HashMap<u64, Address>,
    tokens: HashMap<u64, LockTokens>,
    eurc_price: CoinPrice,
    /// Prices of the priced tokens, by CoinGecko id.
    prices: HashMap<String, CoinPrice>,
}

/// The tokens Bob accepts in a chain's SourceLock, from chains.json.
#[derive(Debug, Clone)]
struct LockTokens {
    usdc: Address,
    /// `None` if chains.json lists no EURC for the chain; EURC locks there are then skipped.
    eurc: Option<Address>,
    /// Tokens with a `priceId` (WETH, WBTC, ...), valued at their market price.
    priced: Vec<PricedToken>,
}

#[derive(Debug, Clone)]
struct PricedToken {
    address: Address,
    decimals: u8,
    price_id: String,
}

impl LockTokens {
    /// How Bob values `token`, and its CoinGecko id if it is a priced token. `None`: not accepted.
    fn kind(&self, token: Address) -> Option<(LockedToken, Option<&str>)> {
        if token == self.usdc {
            Some((LockedToken::Usdc, None))
        } else if Some(token) == self.eurc {
            Some((LockedToken::Eurc, None))
        } else {
            self.priced.iter().find(|t| t.address == token).map(|t| {
                (
                    LockedToken::Priced {
                        decimals: t.decimals,
                    },
                    Some(t.price_id.as_str()),
                )
            })
        }
    }
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
        let watched =
            |chain_id: u64, provider: DynProvider, confirmations: u64, role: Role, contracts| {
                Watched {
                    chain_id,
                    provider,
                    confirmations: config
                        .confirmations
                        .get(&chain_id)
                        .copied()
                        .unwrap_or(confirmations),
                    use_safe_tag: role == Role::Customer
                        && config.safe_tag_chains.contains(&chain_id),
                    contracts,
                    range: AtomicU64::new(config.max_block_range),
                    ceiling: AtomicU64::new(config.max_block_range),
                    send_lock: Mutex::new(()),
                }
            };

        let mut chains = vec![];
        let mut shop_contracts = vec![
            (payout_pool, Source::PayoutPool),
            (destination_lock, Source::DestinationLock),
        ];
        if let Some(direct_pay) = shop.contracts.direct_pay {
            shop_contracts.push((direct_pay, Source::DirectPay));
        }
        chains.push(watched(
            shop.chain_id,
            connect(shop.chain_id, &shop.rpc)?,
            shop.confirmations,
            shop.role,
            shop_contracts,
        ));

        let mut source_locks = HashMap::new();
        let mut tokens = HashMap::new();
        for chain in config.chains.customer_chains() {
            let Some(lock) = chain.contracts.source_lock else {
                continue;
            };
            let token = chain
                .tokens
                .get("USDC")
                .ok_or(format!("chains.json: chain {} has no USDC", chain.chain_id))?;
            let eurc = chain.tokens.get("EURC");
            // Valuing a lock assumes USDC, EURC and USDT all have 6 decimals.
            for t in std::iter::once(token).chain(eurc) {
                if t.decimals != 6 {
                    return Err(format!(
                        "chains.json: token {} on chain {} does not have 6 decimals",
                        t.address, chain.chain_id
                    ));
                }
            }
            if eurc.is_none() {
                eprintln!(
                    "matchmaker: chains.json lists no EURC for chain {}; EURC locks there are skipped",
                    chain.chain_id
                );
            }
            let priced: Vec<PricedToken> = chain
                .tokens
                .iter()
                .filter(|(symbol, _)| !["USDC", "EURC"].contains(&symbol.as_str()))
                .filter_map(|(_, t)| {
                    t.price_id.as_ref().map(|id| PricedToken {
                        address: t.address,
                        decimals: t.decimals,
                        price_id: id.clone(),
                    })
                })
                .collect();
            let names: Vec<&str> = chain
                .tokens
                .iter()
                .filter(|(_, t)| priced.iter().any(|p| p.address == t.address))
                .map(|(symbol, _)| symbol.as_str())
                .collect();
            eprintln!(
                "matchmaker: chain {} accepts USDC{}{}{}",
                chain.chain_id,
                if eurc.is_some() { ", EURC" } else { "" },
                if names.is_empty() { "" } else { ", " },
                names.join(", ")
            );
            source_locks.insert(chain.chain_id, lock);
            tokens.insert(
                chain.chain_id,
                LockTokens {
                    usdc: token.address,
                    eurc: eurc.map(|t| t.address),
                    priced,
                },
            );
            chains.push(watched(
                chain.chain_id,
                connect(chain.chain_id, &chain.rpc)?,
                chain.confirmations,
                chain.role,
                vec![(lock, Source::SourceLock)],
            ));
        }

        let eurc_price = CoinPrice::new(
            config.eurc_price_url.clone(),
            "euro-coin",
            price::EURC_SANE_RANGE,
        )?;
        let mut prices = HashMap::new();
        for t in tokens.values().flat_map(|t: &LockTokens| &t.priced) {
            if !prices.contains_key(&t.price_id) {
                let url = price::simple_price_url(&config.price_api_url, &t.price_id);
                let coin = CoinPrice::new(url, &t.price_id, price::ANY_PRICE)?;
                prices.insert(t.price_id.clone(), coin);
            }
        }
        let mut worker = Worker {
            shop_chain_id: shop.chain_id,
            config,
            store,
            desk,
            filler,
            chains,
            usdt,
            payout_pool,
            destination_lock,
            aqua,
            strategy_hash: B256::ZERO,
            source_locks,
            tokens,
            eurc_price,
            prices,
        };
        worker.strategy_hash = worker.check_setup().await?;
        Ok(worker)
    }

    /// Refuses to start against the wrong contracts or a strategy Bob never shipped. Returns the
    /// strategy's hash.
    async fn check_setup(&self) -> Result<B256> {
        let aqua = self.aqua;
        let shop = self.provider(self.shop_chain_id)?;
        let pool = PayoutPool::new(self.payout_pool, shop);
        let wired = pool.destinationLock().call().await.map_err(err)?;
        if wired != self.destination_lock {
            return Err(format!(
                "chains.json destinationLock {} is not the pool's lock {wired}",
                self.destination_lock
            ));
        }
        let probe = B256::repeat_byte(0x42);
        let on_chain_id = DestinationLock::new(self.destination_lock, shop)
            .lockIdFor(U256::from(84532u64), probe, self.config.provider.address)
            .call()
            .await
            .map_err(err)?;
        let local_id = destination_lock_id(
            self.shop_chain_id,
            self.destination_lock,
            84532,
            probe,
            self.config.provider.address,
        );
        if on_chain_id != local_id {
            return Err("DestinationLock.lockIdFor does not match the local calculation".into());
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
                "The provider's strategy {hash} is not active on Aqua: check BOB_ADDRESS, STRATEGY_SALT, STRATEGY_MAX_PER_ORDER and STRATEGY_FEE_BPS"
            ));
        }
        eprintln!(
            "matchmaker: strategy {hash} active, allowance {}",
            shipped.balance
        );
        for (&chain_id, &lock_address) in &self.source_locks {
            let code = self
                .provider(chain_id)?
                .get_code_at(lock_address)
                .await
                .map_err(err)?;
            if code.is_empty() {
                return Err(format!(
                    "chains.json sourceLock {lock_address} on chain {chain_id} has no code: deploy it or set it to \"\""
                ));
            }
            let watched = self.watched(chain_id)?;
            eprintln!(
                "matchmaker: chain {chain_id} reads up to {}; filler gas floor {} wei",
                if watched.use_safe_tag {
                    "the safe block".to_string()
                } else {
                    format!("{} confirmations", watched.confirmations)
                },
                self.config.min_filler_gas_wei.get(&chain_id).copied().unwrap_or(0)
            );
            let lock = SourceLock::new(lock_address, self.provider(chain_id)?);
            let usdc = lock.usdc().call().await.map_err(err)?;
            let alt = lock.altToken().call().await.map_err(err)?;
            let tokens = &self.tokens[&chain_id];
            if usdc != tokens.usdc || tokens.eurc.is_some_and(|eurc| eurc != alt) {
                return Err(format!(
                    "SourceLock {lock_address} on chain {chain_id} accepts {usdc} and {alt}, not the USDC and EURC in chains.json"
                ));
            }
            for token in std::iter::once(usdc).chain(tokens.eurc) {
                self.require_six_decimals(chain_id, token).await?;
            }
            // A priced lock is valued in the token's own decimals, so they must match chains.json.
            for t in &tokens.priced {
                self.require_decimals(chain_id, t.address, t.decimals).await?;
            }
        }
        self.require_six_decimals(self.shop_chain_id, self.usdt)
            .await?;
        Ok(hash)
    }

    /// The parts of what Bob can pay out now: his strategy's balance on Aqua, the USDT in his
    /// wallet and his approval to Aqua.
    async fn liquidity(&self) -> Result<(U256, U256, U256)> {
        let shop = self.provider(self.shop_chain_id)?;
        let bob = self.config.provider.address;
        let shipped = IAqua::new(self.aqua, shop)
            .rawBalances(bob, self.payout_pool, self.strategy_hash, self.usdt)
            .call()
            .await
            .map_err(err)?;
        // A docked strategy keeps no balance.
        let strategy_balance = if shipped.tokensCount == u8::MAX {
            U256::ZERO
        } else {
            U256::from(shipped.balance)
        };
        let usdt = IERC20::new(self.usdt, shop);
        let wallet = usdt.balanceOf(bob).call().await.map_err(err)?;
        let approved = usdt.allowance(bob, self.aqua).call().await.map_err(err)?;
        Ok((strategy_balance, wallet, approved))
    }

    /// Valuing a lock assumes USDC, EURC and USDT all have 6 decimals; refuse a token that doesn't.
    async fn require_six_decimals(&self, chain_id: u64, token: Address) -> Result<()> {
        self.require_decimals(chain_id, token, 6).await
    }

    async fn require_decimals(&self, chain_id: u64, token: Address, expected: u8) -> Result<()> {
        let decimals = IFiatToken::new(token, self.provider(chain_id)?)
            .decimals()
            .call()
            .await
            .map_err(err)?;
        if decimals != expected {
            return Err(format!(
                "token {token} on chain {chain_id} has {decimals} decimals, not {expected}"
            ));
        }
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

    fn watched(&self, chain_id: u64) -> Result<&Watched> {
        self.chains
            .iter()
            .find(|c| c.chain_id == chain_id)
            .ok_or_else(|| format!("not watching chain {chain_id}"))
    }

    fn provider(&self, chain_id: u64) -> Result<&DynProvider> {
        self.watched(chain_id).map(|w| &w.provider)
    }

    /// The chain's latest block time. A node whose latest block is far behind real time is not
    /// trusted, because a stale clock would overstate how long a source lock has left.
    async fn chain_time(&self, chain_id: u64) -> Result<u64> {
        let block = self
            .provider(chain_id)?
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .map_err(err)?
            .ok_or("no latest block")?;
        let time = block.header.timestamp;
        if time.saturating_add(MAX_CLOCK_LAG) < unix_now() {
            return Err(format!(
                "chain {chain_id} node is behind: latest block is {}s old",
                unix_now() - time
            ));
        }
        Ok(time)
    }

    /// The last block to read on a chain: the "safe" block on rollups, otherwise head minus the
    /// confirmations in chains.json.
    async fn readable_head(&self, watched: &Watched) -> Result<Option<u64>> {
        if watched.use_safe_tag {
            // Never fall back to a few confirmations on a rollup: skip this pass instead.
            return match watched
                .provider
                .get_block_by_number(BlockNumberOrTag::Safe)
                .await
            {
                Ok(Some(block)) => Ok(Some(block.header.number)),
                Ok(None) => Err(format!("chain {}: no safe block", watched.chain_id)),
                Err(e) => Err(format!("chain {}: safe block: {}", watched.chain_id, err(e))),
            };
        }
        let head = watched.provider.get_block_number().await.map_err(err)?;
        Ok(head.checked_sub(watched.confirmations))
    }

    /// Simulates, then sends and waits (with a time limit) for the receipt, one transaction at a
    /// time per chain. Only a revert in the simulation counts as `WouldRevert`; network errors are
    /// returned as `Err` to be retried.
    async fn send(&self, chain_id: u64, tx: TransactionRequest) -> Result<Sent> {
        let watched = self.watched(chain_id)?;
        let _one_at_a_time = watched.send_lock.lock().await;
        let tx = tx.with_from(self.filler);
        if let Err(e) = watched.provider.call(tx.clone()).await {
            let reverted = e
                .as_error_resp()
                .is_some_and(|r| r.as_revert_data().is_some() || r.message.contains("revert"));
            return if reverted {
                Ok(Sent::WouldRevert(err(e)))
            } else {
                Err(err(e))
            };
        }
        let pending = watched.provider.send_transaction(tx).await.map_err(err)?;
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

    /// The main pass. A failure on one chain or one item never stops the others.
    pub async fn tick(&self) {
        for i in 0..self.chains.len() {
            // Read range after range until caught up, so a backlog (first start, an outage)
            // clears in one pass instead of one range per poll.
            for _ in 0..MAX_RANGES_PER_PASS {
                match self.scan(i).await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(e) => {
                        eprintln!("matchmaker: chain {}: {e}", self.chains[i].chain_id);
                        break;
                    }
                }
            }
        }
        self.retry_parked_logs().await;
        self.flush_outbox().await;
        self.refund_expired_destinations().await;
        self.refund_expired_sources().await;
    }

    /// Reads the next confirmed block range on one chain. A log whose handling fails is parked for
    /// retry, so it never holds up the cursor. The cursor only moves once the node is known to have
    /// the range's last block.
    /// Reads and handles one range of logs on a chain. Returns whether more blocks are ready.
    async fn scan(&self, index: usize) -> Result<bool> {
        let watched = &self.chains[index];
        let Some(safe) = self.readable_head(watched).await? else {
            return Ok(false);
        };
        let from = match self.store.cursor(watched.chain_id).await.map_err(err)? {
            Some(next) => next,
            None => safe.saturating_sub(FIRST_RUN_LOOKBACK),
        };
        if from > safe {
            return Ok(false);
        }
        let range = watched.range.load(Ordering::Relaxed).max(1);
        let to = safe.min(from.saturating_add(range - 1));
        if watched
            .provider
            .get_block_by_number(to.into())
            .await
            .map_err(err)?
            .is_none()
        {
            return Ok(false);
        }
        let addresses: Vec<Address> = watched.contracts.iter().map(|(a, _)| *a).collect();
        let filter = Filter::new()
            .address(addresses)
            .from_block(from)
            .to_block(to);
        let mut logs = match watched.provider.get_logs(&filter).await {
            Ok(logs) => {
                let grown = range
                    .saturating_mul(2)
                    .min(watched.ceiling.load(Ordering::Relaxed))
                    .max(1);
                watched.range.store(grown, Ordering::Relaxed);
                logs
            }
            Err(e) => {
                let message = err(e);
                if is_range_error(&message) && range > 1 {
                    watched.ceiling.store(range / 2, Ordering::Relaxed);
                    watched.range.store(range / 2, Ordering::Relaxed);
                }
                return Err(format!("get_logs {from}..={to}: {message}"));
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
                let (Some(tx), Some(log_index)) = (log.transaction_hash, log.log_index) else {
                    return Err(e);
                };
                eprintln!("matchmaker: parking log {tx}:{log_index} for retry: {e}");
                self.store
                    .park_log(&PendingLog {
                        chain_id: watched.chain_id,
                        tx_hash: tx.to_string(),
                        log_index,
                        source: source.as_str().into(),
                        log_json: serde_json::to_string(log).map_err(err)?,
                    })
                    .await
                    .map_err(err)?;
            }
        }
        self.store
            .set_cursor(watched.chain_id, to + 1)
            .await
            .map_err(err)?;
        Ok(to < safe)
    }

    /// Retries every parked log. Outages are retried until they clear; a log that fails for a
    /// permanent reason is dropped after `MAX_LOG_FAILURES` tries.
    async fn retry_parked_logs(&self) {
        let parked = match self.store.pending_logs().await {
            Ok(parked) => parked,
            Err(e) => return eprintln!("matchmaker: parked logs: {}", err(e)),
        };
        for p in parked {
            let (Ok(log), Some(source)) = (
                serde_json::from_str::<Log>(&p.log_json),
                Source::parse(&p.source),
            ) else {
                let _ = self
                    .store
                    .drop_pending(p.chain_id, &p.tx_hash, p.log_index)
                    .await;
                continue;
            };
            match self.handle(p.chain_id, source, &log).await {
                Ok(()) => {
                    let _ = self
                        .store
                        .drop_pending(p.chain_id, &p.tx_hash, p.log_index)
                        .await;
                }
                Err(e) if e.starts_with(PERMANENT) => {
                    let failures = self
                        .store
                        .bump_pending(p.chain_id, &p.tx_hash, p.log_index)
                        .await
                        .unwrap_or(0);
                    if failures >= MAX_LOG_FAILURES {
                        eprintln!(
                            "matchmaker: GIVING UP on log {}:{} after {failures} tries: {e}",
                            p.tx_hash, p.log_index
                        );
                        let _ = self
                            .store
                            .drop_pending(p.chain_id, &p.tx_hash, p.log_index)
                            .await;
                    }
                }
                Err(e) => eprintln!(
                    "matchmaker: parked log {}:{} still failing: {e}",
                    p.tx_hash, p.log_index
                ),
            }
        }
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

    /// A customer locked on a source chain for Bob. The order's legs say which source locks pay
    /// for it: one, or one per chain for a split payment. Once every one of them has been seen,
    /// and together they pass every rule, open one destination lock for the shop with Bob's USDT,
    /// naming the first of them. If Bob already filled it (an earlier run stopped before
    /// recording), record it from the chain.
    async fn on_locked(&self, chain_id: u64, locked: SourceLock::Locked) -> Result<()> {
        let bob = self.config.provider.address;
        if locked.receiver != bob || !self.source_locks.contains_key(&chain_id) {
            return Ok(());
        }
        self.store
            .mark_seen(chain_id, locked.lockId)
            .await
            .map_err(err)?;
        let order = match self.desk.order(locked.orderId).await? {
            OrderLookup::Found(order) => order,
            OrderLookup::NotFound => {
                eprintln!(
                    "matchmaker: skip lock {}: order {} is unknown",
                    locked.lockId, locked.orderId
                );
                return Ok(());
            }
        };
        let this = OrderLeg {
            chain_id,
            lock_id: locked.lockId,
            expiry: locked.expiry,
        };
        let Some(legs) = legs_for(&order, this) else {
            eprintln!(
                "matchmaker: skip lock {}: not one of the locks signed for order {}",
                locked.lockId, locked.orderId
            );
            return Ok(());
        };
        if let Some(leg) = legs
            .iter()
            .find(|l| !self.source_locks.contains_key(&l.chain_id))
        {
            eprintln!(
                "matchmaker: skip order {}: it has a lock on chain {}, which the provider does not watch",
                locked.orderId, leg.chain_id
            );
            return Ok(());
        }
        let anchor = legs[0];
        let shop = self.provider(self.shop_chain_id)?;
        let destination = DestinationLock::new(self.destination_lock, shop);
        let destination_lock_id = destination_lock_id(
            self.shop_chain_id,
            self.destination_lock,
            anchor.chain_id,
            anchor.lock_id,
            bob,
        );
        let existing = destination
            .locks(destination_lock_id)
            .call()
            .await
            .map_err(err)?;
        if existing.state != NONE {
            return self
                .record_existing_fill(&locked, &legs, destination_lock_id, existing)
                .await;
        }
        if self.store.has_fill(locked.orderId).await.map_err(err)? {
            eprintln!(
                "matchmaker: skip lock {}: order {} was already filled",
                locked.lockId, locked.orderId
            );
            return Ok(());
        }
        for leg in legs.iter().filter(|l| **l != this) {
            if !self
                .store
                .seen(leg.chain_id, leg.lock_id)
                .await
                .map_err(err)?
            {
                eprintln!(
                    "matchmaker: lock {} for order {} seen; waiting for its lock on chain {}",
                    locked.lockId, locked.orderId, leg.chain_id
                );
                return Ok(());
            }
        }

        let mut reads = Vec::with_capacity(legs.len());
        for leg in &legs {
            let on_chain = SourceLock::new(
                self.source_locks[&leg.chain_id],
                self.provider(leg.chain_id)?,
            )
            .locks(leg.lock_id)
            .call()
            .await
            .map_err(err)?;
            if on_chain.state == NONE {
                return Err(format!(
                    "source lock {} reads as empty; the node may be behind",
                    leg.lock_id
                ));
            }
            let Some((kind, price_id)) = self.tokens[&leg.chain_id].kind(on_chain.token) else {
                eprintln!(
                    "matchmaker: skip lock {}: token {} is not one the provider accepts here",
                    leg.lock_id, on_chain.token
                );
                return Ok(());
            };
            reads.push(LegRead {
                leg: *leg,
                kind,
                token: on_chain.token,
                price_id: price_id.map(str::to_string),
                view: SourceLockView {
                    open: on_chain.state == OPEN,
                    receiver: on_chain.receiver,
                    usdc_value: U256::from(on_chain.amount),
                    hashlock: on_chain.hashlock,
                    expiry: on_chain.expiry,
                },
                src_now: self.chain_time(leg.chain_id).await?,
            });
        }
        let dest_now = self.chain_time(self.shop_chain_id).await?;
        let decide = |views: Vec<(SourceLockView, u64)>| {
            let combined = rules::combine(&views)?;
            // `combine` measures time left on each lock's own chain, so its expiry is relative.
            rules::fill_decision(
                &combined,
                &order.view,
                &self.config.provider,
                false,
                0,
                dest_now,
                self.config.timing,
            )
            .map(|plan| (plan, combined))
        };
        if reads.iter().any(|r| r.kind.is_priced()) {
            // Every other rule first, so locks that would be skipped anyway never wait on the
            // price. A price outage is retried like any other: the log stays parked until it
            // clears or the locks no longer pass these rules.
            let unpriced = reads
                .iter()
                .map(|r| {
                    let mut view = r.view;
                    if r.kind.is_priced() {
                        view.usdc_value = U256::MAX;
                    }
                    (view, r.src_now)
                })
                .collect();
            if let Err(skip) = decide(unpriced) {
                eprintln!("matchmaker: skip order {}: {skip:?}", locked.orderId);
                return Ok(());
            }
            for r in reads.iter_mut().filter(|r| r.kind.is_priced()) {
                let price = match &r.price_id {
                    None => self.eurc_price.micro_usd().await?,
                    Some(id) => {
                        self.prices
                            .get(id)
                            .ok_or(format!("no price feed for {id}"))?
                            .micro_usd()
                            .await?
                    }
                };
                r.view.usdc_value = rules::usdc_value(
                    r.kind,
                    r.view.usdc_value,
                    price,
                    self.config.price_buffer_bps,
                )
                .ok_or(format!("{PERMANENT}cannot value lock {}", r.leg.lock_id))?;
            }
        }
        let (plan, combined) = match decide(reads.iter().map(|r| (r.view, r.src_now)).collect()) {
            Ok(decided) => decided,
            Err(skip) => {
                eprintln!("matchmaker: skip order {}: {skip:?}", locked.orderId);
                return Ok(());
            }
        };
        for r in &reads {
            // Circle can block an address for USDC or EURC; Bob is paid in the token that was
            // locked. Priced tokens (WETH, WBTC, ...) have no blacklist.
            let circle = matches!(r.kind, LockedToken::Usdc | LockedToken::Eurc);
            if circle
                && IFiatToken::new(r.token, self.provider(r.leg.chain_id)?)
                    .isBlacklisted(bob)
                    .call()
                    .await
                    .map_err(err)?
            {
                eprintln!(
                    "matchmaker: skip lock {}: Circle has blacklisted the provider, it could not redeem",
                    r.leg.lock_id
                );
                return Ok(());
            }
            let gas = self
                .provider(r.leg.chain_id)?
                .get_balance(self.filler)
                .await
                .map_err(err)?;
            let min_gas = self
                .config
                .min_filler_gas_wei
                .get(&r.leg.chain_id)
                .copied()
                .unwrap_or(u128::MAX);
            if gas < U256::from(min_gas) {
                return Err(format!(
                    "filler has too little gas on chain {} to redeem later ({gas} wei); top it up",
                    r.leg.chain_id
                ));
            }
        }

        // Checked last, after every reason to skip. A short pool is an error, so the lock stays
        // parked and is tried again: a top-up before the fill cutoff still fills the order.
        let (strategy_balance, wallet, approved) = self.liquidity().await?;
        let available = rules::liquidity(strategy_balance, wallet, approved);
        if available < plan.amount {
            return Err(format!(
                "not enough liquidity for order {}: needs {} USDT units, the provider can pay {available} (strategy {strategy_balance}, wallet {wallet}, approved to Aqua {approved}); top up the provider's pool",
                locked.orderId, plan.amount
            ));
        }
        let fill = PayoutPool::Fill {
            orderId: locked.orderId,
            merchant: plan.merchant,
            amount: plan.amount,
            hashlock: combined.hashlock,
            expiry: plan.destination_expiry,
            srcChainId: U256::from(anchor.chain_id),
            srcLockId: anchor.lock_id,
        };
        let tx = PayoutPool::new(self.payout_pool, shop)
            .fill(self.strategy()?, fill)
            .into_transaction_request();
        match self.send(self.shop_chain_id, tx).await? {
            Sent::WouldRevert(reason) => {
                eprintln!(
                    "matchmaker: skip order {}: fill would revert: {reason}",
                    locked.orderId
                );
                Ok(())
            }
            Sent::Mined(hash) => {
                eprintln!(
                    "matchmaker: filled order {} ({} source lock(s)) in {hash}",
                    locked.orderId,
                    reads.len()
                );
                let sources: Vec<SourceLeg> = reads
                    .iter()
                    .map(|r| SourceLeg {
                        chain_id: r.leg.chain_id,
                        lock_id: r.leg.lock_id,
                        expiry: r.view.expiry,
                    })
                    .collect();
                self.store
                    .record_fill_if_missing(
                        &Fill {
                            order_id: locked.orderId,
                            src_chain_id: anchor.chain_id,
                            src_lock_id: anchor.lock_id,
                            src_expiry: reads[0].view.expiry,
                            destination_lock_id,
                            destination_expiry: plan.destination_expiry,
                            amount: plan.amount,
                            status: "filled".into(),
                            secret: None,
                        },
                        &sources,
                    )
                    .await
                    .map_err(err)?;
                Ok(())
            }
        }
    }

    /// Records a destination lock Bob opened in an earlier run that stopped before recording it,
    /// with every source lock it pays for.
    async fn record_existing_fill(
        &self,
        locked: &SourceLock::Locked,
        legs: &[OrderLeg],
        destination_lock_id: B256,
        existing: DestinationLock::locksReturn,
    ) -> Result<()> {
        if existing.maker != self.config.provider.address || existing.hashlock != locked.hashlock {
            return Err(format!(
                "{PERMANENT}destination lock {destination_lock_id} does not match source lock {}",
                locked.lockId
            ));
        }
        let secret = if existing.state == CLAIMED {
            self.find_secret(destination_lock_id).await?
        } else {
            None
        };
        let status = match (existing.state, secret) {
            (CLAIMED, Some(_)) => "settled",
            (OPEN | CLAIMED, _) => "filled",
            _ => "refunded",
        };
        let anchor = legs[0];
        let new = self
            .store
            .record_fill_if_missing(
                &Fill {
                    order_id: locked.orderId,
                    src_chain_id: anchor.chain_id,
                    src_lock_id: anchor.lock_id,
                    src_expiry: anchor.expiry,
                    destination_lock_id,
                    destination_expiry: existing.expiry,
                    amount: U256::from(existing.amount),
                    status: status.into(),
                    secret,
                },
                &source_legs(legs),
            )
            .await
            .map_err(err)?;
        if new {
            eprintln!(
                "matchmaker: recorded an earlier fill for order {} from the chain",
                locked.orderId
            );
        }
        Ok(())
    }

    /// The secret revealed for one of our destination locks, read from its Settled log.
    async fn find_secret(&self, lock_id: B256) -> Result<Option<B256>> {
        let provider = self.provider(self.shop_chain_id)?;
        let chunk = self.config.max_block_range.max(1);
        let head = provider.get_block_number().await.map_err(err)?;
        let floor = head.saturating_sub(SECRET_SEARCH_BLOCKS);
        let mut to = head;
        loop {
            let from = to.saturating_sub(chunk - 1).max(floor);
            let filter = Filter::new()
                .address(self.destination_lock)
                .event_signature(DestinationLock::Settled::SIGNATURE_HASH)
                .topic1(lock_id)
                .from_block(from)
                .to_block(to);
            for log in provider.get_logs(&filter).await.map_err(err)? {
                if let Ok(settled) = DestinationLock::Settled::decode_log(&log.inner) {
                    return Ok(Some(settled.secret));
                }
            }
            if from <= floor {
                return Ok(None);
            }
            to = from - 1;
        }
    }

    /// The shop was paid on Whitechain and the secret is public. The log came from our
    /// DestinationLock. A fill that was never recorded (a run stopped right after filling) is
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
        if destination_lock_id(
            self.shop_chain_id,
            self.destination_lock,
            src_chain_id,
            settled.srcLockId,
            bob,
        ) != settled.lockId
        {
            return Ok(()); // someone else's destination lock
        }
        let source = SourceLock::new(lock_address, self.provider(src_chain_id)?)
            .locks(settled.srcLockId)
            .call()
            .await
            .map_err(err)?;
        if source.state == NONE {
            return Err(format!(
                "source lock {} reads as empty; the node may be behind",
                settled.srcLockId
            ));
        }
        if source.receiver != bob {
            return Ok(());
        }
        let existing =
            DestinationLock::new(self.destination_lock, self.provider(self.shop_chain_id)?)
                .locks(settled.lockId)
                .call()
                .await
                .map_err(err)?;
        let anchor = OrderLeg {
            chain_id: src_chain_id,
            lock_id: settled.srcLockId,
            expiry: source.expiry,
        };
        // A split payment's other source locks are listed only by the Order Desk.
        let legs = match self.desk.order(settled.orderId).await? {
            OrderLookup::Found(order) => legs_for(&order, anchor),
            OrderLookup::NotFound => None,
        }
        .unwrap_or_else(|| {
            eprintln!(
                "matchmaker: CHECK order {}: the Order Desk no longer lists source lock {}; recording only that one, so any other lock of this payment must be redeemed by hand",
                settled.orderId, settled.srcLockId
            );
            vec![anchor]
        });
        let new = self
            .store
            .record_fill_if_missing(
                &Fill {
                    order_id: settled.orderId,
                    src_chain_id,
                    src_lock_id: settled.srcLockId,
                    src_expiry: source.expiry,
                    destination_lock_id: settled.lockId,
                    destination_expiry: existing.expiry,
                    amount: settled.amount,
                    status: "settled".into(),
                    secret: Some(settled.secret),
                },
                &source_legs(&legs),
            )
            .await
            .map_err(err)?;
        if new {
            eprintln!(
                "matchmaker: recorded an unrecorded fill for order {} from its Settled log",
                settled.orderId
            );
        }
        Ok(())
    }

    /// The redeem loop's pass: notice destination locks paid to the shop (even if their Settled log
    /// was missed), then redeem every settled fill's source lock for Bob.
    pub async fn redeem_duty(&self) {
        match self.store.fills_with_status("filled").await {
            Ok(fills) => {
                for fill in fills {
                    if let Err(e) = self.check_filled(&fill).await {
                        eprintln!("matchmaker: checking fill for order {}: {e}", fill.order_id);
                    }
                }
            }
            Err(e) => eprintln!("matchmaker: checking fills: {}", err(e)),
        }
        match self.store.fills_with_status("settled").await {
            Ok(fills) => {
                for fill in fills {
                    if let Err(e) = self.redeem(&fill).await {
                        eprintln!("matchmaker: source redeem for order {}: {e}", fill.order_id);
                    }
                }
            }
            Err(e) => eprintln!("matchmaker: source redeem: {}", err(e)),
        }
    }

    /// A destination lock that was paid to the shop moves its fill to settled with the secret.
    async fn check_filled(&self, fill: &Fill) -> Result<()> {
        let state = DestinationLock::new(self.destination_lock, self.provider(self.shop_chain_id)?)
            .locks(fill.destination_lock_id)
            .call()
            .await
            .map_err(err)?
            .state;
        match state {
            CLAIMED => match self.find_secret(fill.destination_lock_id).await? {
                Some(secret) => self
                    .store
                    .mark_settled(fill.destination_lock_id, secret)
                    .await
                    .map_err(err),
                None => Err(format!(
                    "destination lock {} was claimed but its Settled log was not found yet",
                    fill.destination_lock_id
                )),
            },
            NONE => Err(format!(
                "destination lock {} reads as empty; the node may be behind",
                fill.destination_lock_id
            )),
            _ => Ok(()),
        }
    }

    /// Redeems every open source lock of a settled fill with its secret. Once none is open, the
    /// fill is claimed, or missed if any lock could not be redeemed.
    async fn redeem(&self, fill: &Fill) -> Result<()> {
        let Some(secret) = fill.secret else {
            return Ok(());
        };
        let legs = self
            .store
            .legs(fill.destination_lock_id)
            .await
            .map_err(err)?;
        let mut first_error = None;
        for leg in legs.iter().filter(|l| l.status == "open") {
            if let Err(e) = self.redeem_leg(leg, secret).await {
                eprintln!(
                    "matchmaker: source redeem for lock {}: {e}",
                    leg.src_lock_id
                );
                first_error.get_or_insert(e);
            }
        }
        if let Some(e) = first_error {
            return Err(e);
        }
        let legs = self
            .store
            .legs(fill.destination_lock_id)
            .await
            .map_err(err)?;
        if legs.is_empty() || legs.iter().any(|l| l.status == "open") {
            return Ok(());
        }
        let status = if legs.iter().all(|l| l.status == "claimed") {
            "claimed"
        } else {
            "missed"
        };
        self.store
            .set_fill_status(fill.destination_lock_id, status)
            .await
            .map_err(err)
    }

    async fn redeem_leg(&self, leg: &FillLeg, secret: B256) -> Result<()> {
        let Some(&lock_address) = self.source_locks.get(&leg.src_chain_id) else {
            // Not a chain this Matchmaker watches (its config changed): it cannot redeem here.
            return self.mark_missed(leg).await;
        };
        let lock = SourceLock::new(lock_address, self.provider(leg.src_chain_id)?);
        let on_chain = lock.locks(leg.src_lock_id).call().await.map_err(err)?;
        match on_chain.state {
            CLAIMED => {
                return self.store.set_leg_status(leg, "claimed").await.map_err(err);
            }
            REFUNDED => return self.mark_missed(leg).await,
            OPEN => {}
            _ => {
                return Err(format!(
                    "source lock {} reads as empty; the node may be behind",
                    leg.src_lock_id
                ));
            }
        }
        let now = self.chain_time(leg.src_chain_id).await?;
        if now >= on_chain.expiry {
            // Confirm with a second read before giving up the redeem.
            let again = lock.locks(leg.src_lock_id).call().await.map_err(err)?;
            let now = self.chain_time(leg.src_chain_id).await?;
            if again.state == OPEN && now >= again.expiry {
                return self.mark_missed(leg).await;
            }
            return Ok(());
        }
        match self
            .send(
                leg.src_chain_id,
                lock.claim(leg.src_lock_id, secret)
                    .into_transaction_request(),
            )
            .await?
        {
            Sent::Mined(hash) => {
                eprintln!(
                    "matchmaker: redeemed source lock {} in {hash}",
                    leg.src_lock_id
                );
                self.store.set_leg_status(leg, "claimed").await.map_err(err)
            }
            Sent::WouldRevert(reason) => Err(format!("claim would revert: {reason}")),
        }
    }

    async fn mark_missed(&self, leg: &FillLeg) -> Result<()> {
        eprintln!(
            "matchmaker: MISSED source lock {} for order {}: the shop was paid but the lock expired or was refunded",
            leg.src_lock_id, leg.order_id
        );
        self.store.set_leg_status(leg, "missed").await.map_err(err)
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
        for fill in fills
            .into_iter()
            .filter(|f| f.destination_expiry <= now)
            .take(REFUNDS_PER_PASS)
        {
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
                        .set_fill_status(fill.destination_lock_id, "refunded")
                        .await
                        .map_err(err)
                }
                Sent::WouldRevert(reason) => Err(format!("refund would revert: {reason}")),
            },
            REFUNDED => self
                .store
                .set_fill_status(fill.destination_lock_id, "refunded")
                .await
                .map_err(err),
            CLAIMED => Ok(()), // the redeem loop moves it to settled
            _ => Err(format!(
                "destination lock {} reads as empty; the node may be behind",
                fill.destination_lock_id
            )),
        }
    }

    /// Source locks of Bob's fills that timed out unredeemed: send the tokens back to the customer.
    /// Only locks Bob filled are handled, so nobody can make Bob pay gas for locks he never used.
    async fn refund_expired_sources(&self) {
        for (&chain_id, &lock_address) in &self.source_locks {
            let now = match self.chain_time(chain_id).await {
                Ok(now) => now,
                Err(e) => {
                    eprintln!("matchmaker: source refunds on chain {chain_id}: {e}");
                    continue;
                }
            };
            let due = match self
                .store
                .source_refunds_due(chain_id, now, REFUNDS_PER_PASS as u32)
                .await
            {
                Ok(due) => due,
                Err(e) => {
                    eprintln!("matchmaker: source refunds on chain {chain_id}: {}", err(e));
                    continue;
                }
            };
            for leg in due {
                if let Err(e) = self.refund_source(chain_id, lock_address, &leg, now).await {
                    eprintln!(
                        "matchmaker: source refund for lock {}: {e}",
                        leg.src_lock_id
                    );
                }
            }
        }
    }

    async fn refund_source(
        &self,
        chain_id: u64,
        lock_address: Address,
        leg: &FillLeg,
        now: u64,
    ) -> Result<()> {
        let lock = SourceLock::new(lock_address, self.provider(chain_id)?);
        let on_chain = lock.locks(leg.src_lock_id).call().await.map_err(err)?;
        match on_chain.state {
            OPEN if on_chain.expiry <= now => {}
            OPEN => return Ok(()),
            CLAIMED | REFUNDED => {
                return self.store.source_done(leg).await.map_err(err);
            }
            _ => {
                return Err(format!(
                    "source lock {} reads as empty; the node may be behind",
                    leg.src_lock_id
                ));
            }
        }
        match self
            .send(
                chain_id,
                lock.refund(leg.src_lock_id).into_transaction_request(),
            )
            .await?
        {
            Sent::Mined(hash) => {
                eprintln!(
                    "matchmaker: refunded source lock {} in {hash}",
                    leg.src_lock_id
                );
                self.store.source_done(leg).await.map_err(err)
            }
            Sent::WouldRevert(reason) => Err(format!("refund would revert: {reason}")),
        }
    }

    /// Reports queued events in order. A temporary failure stops here to keep the order; an event
    /// the Order Desk refuses for good is moved to `dead_letters` so it cannot block the rest.
    async fn flush_outbox(&self) {
        let items = match self.store.outbox().await {
            Ok(items) => items,
            Err(e) => return eprintln!("matchmaker: reporting: {}", err(e)),
        };
        for (chain_id, tx_hash, log_index, body) in items {
            let done = match self.desk.report(&body).await {
                Ok(()) => self.store.dequeue(chain_id, &tx_hash, log_index).await,
                Err(ReportError::Rejected(reason)) => {
                    eprintln!(
                        "matchmaker: Order Desk refused event {tx_hash}:{log_index}; kept in dead_letters: {reason}"
                    );
                    self.store
                        .dead_letter(chain_id, &tx_hash, log_index, &body, &reason)
                        .await
                }
                Err(ReportError::Retry(reason)) => {
                    return eprintln!("matchmaker: reporting: {reason}");
                }
            };
            if let Err(e) = done {
                return eprintln!("matchmaker: reporting: {}", err(e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn only_the_configured_usdc_and_eurc_are_lock_tokens() {
        let usdc = Address::repeat_byte(1);
        let eurc = Address::repeat_byte(2);
        let both = LockTokens {
            usdc,
            eurc: Some(eurc),
            priced: vec![],
        };
        assert_eq!(both.kind(usdc), Some((LockedToken::Usdc, None)));
        assert_eq!(both.kind(eurc), Some((LockedToken::Eurc, None)));
        assert_eq!(both.kind(Address::repeat_byte(3)), None);
        assert_eq!(both.kind(Address::ZERO), None);
        let usdc_only = LockTokens {
            usdc,
            eurc: None,
            priced: vec![],
        };
        assert_eq!(usdc_only.kind(eurc), None);
    }

    #[test]
    fn priced_tokens_carry_their_decimals_and_price_id_and_unlisted_ones_are_skipped() {
        let weth = Address::repeat_byte(4);
        let wbtc = Address::repeat_byte(5);
        let tokens = LockTokens {
            usdc: Address::repeat_byte(1),
            eurc: None,
            priced: vec![
                PricedToken {
                    address: weth,
                    decimals: 18,
                    price_id: "ethereum".into(),
                },
                PricedToken {
                    address: wbtc,
                    decimals: 8,
                    price_id: "bitcoin".into(),
                },
            ],
        };
        assert_eq!(
            tokens.kind(weth),
            Some((LockedToken::Priced { decimals: 18 }, Some("ethereum")))
        );
        assert_eq!(
            tokens.kind(wbtc),
            Some((LockedToken::Priced { decimals: 8 }, Some("bitcoin")))
        );
        // An ERC-20 chains.json does not list (or lists without a priceId) is not accepted.
        assert_eq!(tokens.kind(Address::repeat_byte(6)), None);
        assert!(LockedToken::Priced { decimals: 8 }.is_priced());
        assert!(LockedToken::Eurc.is_priced());
        assert!(!LockedToken::Usdc.is_priced());
    }

    fn order_with(legs: Vec<OrderLeg>) -> Order {
        Order {
            view: crate::rules::OrderView {
                merchant: Address::repeat_byte(5),
                amount: U256::from(10_000_000u64),
                status: common::status::OrderStatus::Paying,
            },
            legs,
        }
    }

    fn leg(chain_id: u64, byte: u8) -> OrderLeg {
        OrderLeg {
            chain_id,
            lock_id: B256::repeat_byte(byte),
            expiry: 2000,
        }
    }

    #[test]
    fn a_split_order_is_paid_by_all_its_listed_locks_in_a_fixed_order() {
        let order = order_with(vec![leg(84532, 9), leg(11155111, 1)]);
        let from_either = |seen| legs_for(&order, seen).unwrap();
        assert_eq!(from_either(leg(11155111, 1)), from_either(leg(84532, 9)));
        assert_eq!(from_either(leg(11155111, 1))[0], leg(84532, 9));
        assert_eq!(from_either(leg(11155111, 1)).len(), 2);
    }

    #[test]
    fn a_lock_the_order_does_not_list_pays_for_nothing() {
        let order = order_with(vec![leg(84532, 9), leg(11155111, 1)]);
        assert_eq!(legs_for(&order, leg(84532, 2)), None);
        assert_eq!(legs_for(&order, leg(11155111, 9)), None);
    }

    #[test]
    fn an_order_without_listed_locks_is_paid_by_no_lock() {
        assert_eq!(legs_for(&order_with(vec![]), leg(84532, 2)), None);
    }

    #[test]
    fn urls_are_removed_from_error_text() {
        assert_eq!(
            redact_urls(
                "error sending request for url (https://rpc.ankr.com/base_sepolia/SECRETKEY): timeout"
            ),
            "error sending request for url (<rpc>): timeout"
        );
        assert_eq!(redact_urls("no url here"), "no url here");
        assert_eq!(redact_urls("http://a/1 and https://b/2"), "<rpc> and <rpc>");
        assert_eq!(
            redact_urls("a https://x/KEY b http://y/z"),
            "a <rpc> b <rpc>"
        );
    }

    #[test]
    fn only_range_errors_shrink_the_log_range() {
        assert!(is_range_error(
            "query returned more than 10000 results; try with this block range"
        ));
        assert!(is_range_error("block range is too wide"));
        assert!(!is_range_error("429 Too Many Requests: rate limited"));
        assert!(!is_range_error("operation timed out"));
        assert!(!is_range_error("connection reset"));
    }

    #[test]
    fn local_destination_lock_id_matches_the_live_contract() {
        // Checked against DestinationLock.lockIdFor on Whitechain Sepolia (cast call).
        let id = destination_lock_id(
            1874,
            address!("669c928135B848C828144360aD7a73391968BDA3"),
            84532,
            B256::repeat_byte(0x42),
            address!("C2bF8030f00CAB26c4611f93c5F71b9d01Ec120A"),
        );
        assert_eq!(id, EXPECTED_ID);
    }

    const EXPECTED_ID: B256 = alloy::primitives::b256!(
        "6b2a6c18d08a7ee37c0ec7f5b7edee81bb20cc12cf69c7b45f35aa61bd250ba2"
    );
}
