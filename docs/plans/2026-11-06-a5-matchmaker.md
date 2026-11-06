# A5 Matchmaker Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** A Rust service that runs a cross-chain payment for Bob with no human step. It watches a customer's source lock on Base, fills a destination lock for the shop on Whitechain, and redeems the source lock with the secret once the shop is paid. It also refunds anything that times out and reports every chain event to the Order Desk. Done when a manual lock on Base results in the shop being paid and Bob being repaid with no human action (M2).

**What the Matchmaker does, step by step**

```
every few seconds, for each chain, up to (head - confirmations):
  Base      SourceLock.Locked    → report; if it is a valid lock for Bob → fill on Whitechain
  Whitechain PayoutPool.Filled   → report
  Whitechain DestinationLock.Settled (our lock) → report; redeem the source lock with the secret
  Base      SourceLock.Claimed / Refunded, Whitechain DestinationLock.Refunded,
            DirectPay.Paid / Refunded → report
then:
  our destination locks past expiry and still open → DestinationLock.refund (USDT back into Bob's Aqua)
  our source locks past expiry and still open      → SourceLock.refund (USDC back to the customer)
```

**Checks before filling** (from the A3 and A4 reviews):
- The event came from our SourceLock address, on a source chain from `chains.json`.
- The on-chain lock (`SourceLock.locks(lockId)`) is open, its receiver is Bob, and its hashlock isn't zero.
- At least `fillCutoff` (15 min) is left on the source lock. The destination expiry is now plus 10 min, and it ends at least 5 min before the source lock.
- The order exists on the Order Desk (`GET /v1/orders/:id`), is still payable (`quoted`, `paying` or `source_locked`), and the locked USDC covers the order amount plus Bob's fee (`providerFeeBps`, rounded up).
- The amount is at most Bob's `maxPerOrder`, and the order hasn't been filled before (one fill per order).
- The fill is simulated before it is sent.
- Circle has not blacklisted Bob on the source chain, and the filler holds enough gas there to redeem later.

**After the audit and verification reviews:**
- **Recovery first:** before applying the rules, it reads Bob's destination lock for this source lock. If it already exists (an earlier run stopped before recording it), it records it from the chain, whatever the rules or the Order Desk now say. A `Settled` for a fill it never recorded is also recorded, once the chain confirms the lock is Bob's.
- **Clocks:** the destination expiry is set on Whitechain's clock from the time the source lock has left, so a clock difference cannot shrink the 5-minute gap. `fillCutoff` must be at least the destination lock time plus the gap; the config is rejected otherwise.
- **Sending:** the nonce is read from the chain for every transaction (no cached nonce gap), every send is simulated first, and every RPC call and receipt has a time limit. Only a real revert counts as "skip"; network errors are retried.
- **No single item blocks the rest:** each redeem and refund runs on its own. Network and Order Desk outages are retried until they clear; only a permanent error (data on chain that does not match) counts towards skipping a log after 30 tries, and an event the Order Desk refuses for good is dropped. The cursor only moves once the node has the range's last block, and the range halves if the RPC refuses it. Logs removed by a reorg are ignored.
- **Start-up checks:** our DestinationLock is the PayoutPool's own, Bob's strategy is active on Aqua with these settings, and `STRATEGY_FEE_BPS` equals `providerFeeBps`.
- **Missed redeem:** if the source lock expired before Bob redeemed, the fill is marked `missed` and logged loudly.
- **Secret lookup:** if a destination lock was paid to the shop but its `Settled` log was never handled, the secret is read from that lock's `Settled` log on chain (searching back up to 200,000 blocks) and Bob redeems.
- **Timing limits:** `MIN_GAP_SECONDS` must be at least 60 and the destination lock time between 300 and 7200 seconds (the DestinationLock limits).
**After the second review round:**
- **A stale read never gives up money:** a lock that reads as empty is treated as a lagging node and retried. A fill is marked `missed` only when the source lock reads as refunded, or as open and expired on a second read.
- **Separate redeem loop:** source redeems run on their own loop, so slow scans or refunds never delay them. Each round it also checks every open fill on chain, so a destination lock paid to the shop is noticed (and its secret read from the `Settled` log) even if that log was missed.
- **One transaction at a time per chain:** the two loops never race for a nonce.
- **Safe blocks on rollups:** rollup customer chains (`SAFE_TAG_CHAINS`, default Base Sepolia) are read up to their `safe` block, not a few confirmations, so an L2 reorg cannot remove a lock Bob already filled. On Base this adds about a minute before a fill. Ethereum Sepolia waits its `confirmations` from chains.json instead (A7): its safe block trails by about 9 minutes, too long for a 20-minute lock.
- **Log range:** it halves only when the RPC refuses the range (not on rate limits or timeouts), never grows back past a size that was refused, and settles in a few tries (the public Base RPC allows 200 blocks).
- **Failed logs are parked:** a log whose handling fails goes to a retry table, so it never holds up the chain's cursor. Outages are retried until they clear; a permanent mismatch is dropped after 30 tries.
- **Junk locks cost Bob nothing:** a lock for an order the Order Desk doesn't know costs one HTTP request and no gas, and the Matchmaker only refunds source locks Bob actually filled (anyone can refund the others).
- **Store:** fills are keyed by destination lock, so a second fill for an order is never silently dropped; events the Order Desk refuses are kept in `dead_letters`.
- **Checks:** a node whose latest block is more than 2 minutes old is not trusted for time; the gap is at least 300 s; `ORDER_DESK_URL` must use https unless it is on this machine; the token must not be empty; the first run looks back 6,000 blocks (about 3 hours of Base).
- The destination lock id is computed locally and checked against the live contract at start-up.

- **Known limit:** a transaction stuck in the mempool is not re-sent with a higher fee; later transactions from the filler on that chain wait behind it. Fees come from the network's estimate, which is enough on these testnets.
- RPC URLs are removed from error text.

**Not yet strict:** the audit rule "accept only the exact lock id recomputed from the quote" needs the cross-chain quote that B4 adds. Until then, the checks above apply, plus one fill per order. When B4 lands, the Order Desk exposes the expected lock id and the Matchmaker requires an exact match.

**Architecture** (Rust, in the workspace B created):
- `server/bindings`: `alloy::sol!` bindings generated from `shared/abis/*.json` for SourceLock, PayoutPool, DestinationLock and DirectPay.
- `server/matchmaker`:
  - `config.rs`: environment and `chains.json`.
  - `rules.rs`: pure decision functions, unit-tested with no chain.
  - `events.rs`: maps logs to `common::events::ChainEvent`.
  - `store.rs`: its own SQLite file, with `cursors`, `fills` and an `outbox` of events not yet reported.
  - `order_desk.rs`: the HTTP client.
  - `main.rs`: the loop.
- Its database is separate from the Order Desk's Postgres. The only link is HTTP.

**Configuration (environment):**
- `CHAINS_PATH`, `ORDER_DESK_URL`, `INTERNAL_EVENTS_TOKEN`
- `FILLER_PRIVATE_KEY`: the Matchmaker's own wallet. It pays gas on every chain and is the strategy's `filler`.
- Bob's strategy: `BOB_ADDRESS`, `STRATEGY_SALT`, `STRATEGY_MAX_PER_ORDER`, `STRATEGY_FEE_BPS`. These must match what Bob shipped.
- Optional `RPC_<chainId>` overrides (Ankr for Whitechain), `DB_PATH`, `POLL_SECONDS`.

---

### Task 1: Bindings crate
Generate the bindings from `shared/abis/` and check that the crate builds.

### Task 2: Rules (tests first)
Pure functions:
- `required_usdc(order_amount, fee_bps)`
- `fill_decision(lock, order, now, config)`, which returns Fill or Skip(reason)
- `destination_expiry(now, source_expiry, config)`
- the `events` mapping

Unit tests cover every Skip reason and the boundaries.

### Task 3: Store, Order Desk client, and the loop
SQLite schema created on start; on the first run each cursor starts 6,000 blocks before the readable head, so locks made before start-up are still seen. The outbox retries events until the Order Desk accepts them. Logs are read in ranges of at most 1,000 blocks.

### Task 4: Local end to end on two anvil forks (Base and Whitechain) — passed
Harness: `server/matchmaker/e2e/` (see its README). Four scenarios passed: a normal payment, a timeout refund on both sides, a crash after filling with the secret revealed while the Matchmaker was down, and a crash before recording followed by a timeout. Bob's test account had to be a fresh key: anvil's well-known keys carry EIP-7702 code on Base Sepolia, and USDC then treats them as smart wallets.
The contracts are already deployed on both forks. A mock Order Desk serves the order. A script plays the customer: it locks on Base, waits for the fill, then reveals the secret. The Matchmaker must fill, then redeem. Then the same run with no reveal: both sides refund after expiry.

### Task 5: Separate audit agent and verify agent
The audit agent hunts for exploits and failure modes: replay, fake events, reorgs, RPC errors, double fills, and stuck funds. The verify agent checks the code against this plan and the SPEC rules line by line, and re-checks every audit fix.

### Task 6: Live on Base Sepolia and Whitechain — passed (M2)
A 0.99 USDT order settled across the real testnets, with the Matchmaker running against the public RPCs and a stand-in Order Desk:

| Step | Chain | Transaction |
|---|---|---|
| Bob ships a 1 USDT strategy on Aqua | Whitechain | [0xac5a930b…](https://explorer.testnet.whitechain.io/tx/0xac5a930bd05a05433c30e8eba03b66730dcf0ce50b29eaae3e31bd76cc8dd94e) |
| Customer locks 0.99297 USDC for Bob (signature only) | Base Sepolia | [0x4f8baaaf…](https://sepolia.basescan.org/tx/0x4f8baaaffbd7979cd885c8a0630ce61b732dcc54161ce7f2cd3ba03f214cd939) |
| **Matchmaker fills** the destination lock from Bob's strategy | Whitechain | [0xd2961282…](https://explorer.testnet.whitechain.io/tx/0xd29612828c52d6a7bf5865f201cdc0696516ddb1d1e5ddec39570ce4ec72d3ff) |
| Customer reveals the secret: the shop gets 0.99 USDT (`Settled`) | Whitechain | [0xaea41865…](https://explorer.testnet.whitechain.io/tx/0xaea4186570335fc3e72eff00afdbd9fd94adadef817fdcc9f89ab7e103b97339) |
| **Matchmaker redeems** the source lock: Bob gets 0.99297 USDC | Base Sepolia | [0x2de5c351…](https://sepolia.basescan.org/tx/0x2de5c3518350c7057314cf292e0f5413a759f7e648bc92f9f7304a008c508ff6) |

Every event was reported to the Order Desk: Locked, Filled, Settled, Claimed. A first lock was made by mistake with the wrong receiver ([0xa0fc84a7…](https://sepolia.basescan.org/tx/0xa0fc84a73166afc364ffed94251755c0b7f847c9c971499a3dfd18c9b1b7a5b2)); the Matchmaker correctly ignored it, and it was refunded to the customer in full after expiry ([0x1ff83c33…](https://sepolia.basescan.org/tx/0x1ff83c332b78ed30c9cd98798a7409cbd133e9f1a6e5575ae2325131f1c4ce65)).
