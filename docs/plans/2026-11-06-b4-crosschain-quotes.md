# B4: Cross-chain quotes, legs and the Relayer's lock jobs (USDC only)

**Goal:** a customer who holds USDC on Base can pay a shop that is paid in USDT on Whitechain, with no gas. Uniform's Relayer sends every transaction the customer would otherwise have to send: the source lock, the destination claim, and the source refund when Bob never fills.

**Done when:** for a 12 USDT order and a customer holding USDC on Base:
1. `POST /quote` with a hashlock returns a 12.036 USDC note that SourceLock accepts.
2. `POST /legs` hands the signed note to the Relayer, which locks the USDC on Base.
3. Bob's Matchmaker fills on Whitechain.
4. `POST /secret` makes the Relayer claim the destination lock, and the shop is paid.

Tested on Base and Whitechain forks first, then once live. A source lock Bob never fills is refunded by the Relayer after it times out.

**Not in this task:** Uniswap, EURC and ETH (EURC is already accepted by A's SourceLock, but the quote here is USDC only), split payments (Path C), the pay page (B5), shop refunds on the cross-chain path (A9).

## 1. The two locks and the six steps

A cross-chain payment uses two locks. Each ends by being claimed with the secret, or by being refunded after it times out.

| # | Step | Chain | Sent by | Status |
|---|---|---|---|---|
| 1 | Source lock: the customer's USDC, for Bob | Base | **our Relayer** | this task |
| 2 | Destination lock: Bob's USDT, for the shop | Whitechain | Matchmaker | done (A5) |
| 3 | Destination claim: the secret pays the shop | Whitechain | **our Relayer** | this task |
| 4 | Source claim: Bob reuses the secret | Base | Matchmaker | done (A5) |
| 5 | Destination refund: USDT back to Bob | Whitechain | Matchmaker | done (A5) |
| 6 | Source refund: USDC back to the customer | Base | Matchmaker, **only for locks Bob filled**; **our Relayer** for the rest | this task |

The customer has no gas on any chain, so steps 1, 3 and the uncovered half of 6 are ours.

## 2. How it works today

- **Order Desk:** `POST /quote` handles Path A only. `/internal/events` applies `Paid` only; the other events are stored and not applied.
- **Relayer:** one chain (Whitechain), one job (`DirectPay.pay`). Its machinery (sign and save before sending, same-bytes retries, fee bumps, leases, per-shop gas budget) does not depend on the job.
- **SourceLock (A6, Base Sepolia `0x02dF…4871`):** USDC or EURC. `lockWithAuthorization(params, payer, token, amount, validAfter, validBefore, v, r, s, "")`; the note's nonce is `lockIdFor(params, payer, token, amount)` with `LOCK_TAG = keccak256("uniform.sourcelock.lock.v2")`.
- **Matchmaker (A5 on `dev`):** fills a source lock whose receiver is Bob for an order that is `quoted`, `paying` or `source_locked`, when the lock covers the order plus 0.3%, with at least 15 minutes left. It refunds only source locks it filled.

## 3. The flow after B4

1. (B5) The pay page reads the customer's balances, picks the path, and makes the secret and its hashlock.
2. `POST /quote {payer, hashlock, chainId}` → a USDC note for SourceLock, with lock params Bob will accept.
3. The customer signs. `POST /legs {quoteId, signature}` → a leg and a `source_lock` job. Order `paying`.
4. Relayer locks on Base. Matchmaker reports `Locked` (order `source_locked`), fills, reports `Filled` (`destination_locked`).
5. (B5) The pay page checks Bob's lock itself. `POST /secret {secret}` → a `destination_claim` job.
6. Relayer claims on Whitechain. Matchmaker reports `Settled` (`paid`) and redeems the source lock (`Claimed`).
7. If nothing gets filled: after the source lock times out, the Order Desk writes a `source_refund` job and the Relayer refunds. `Refunded` → order `returned`.

## 4. Changes

### 4a. Quote (Order Desk)
- **Request:** `{payer}` gives Path A, as today. `{payer, hashlock, chainId?}` gives the cross-chain path from `chainId`; the default is the one customer chain with a SourceLock.
- **Who picks the path:** the pay page, from balances it reads itself. The Order Desk stays free of chain connections. A wrong choice costs no gas: the Relayer checks the balance and dry-runs before it sends.
- **Amount:** `ceil(order × (10000 + providerFeeBps) / 10000)` USDC, the Matchmaker's own `required_usdc` formula. 12 USDT → 12.036 USDC.
- **Lock params:**
  - `orderId`;
  - `receiver` = `PROVIDER_ADDRESS` (new Order Desk setting: Bob's address);
  - `minUsdcOut` = the amount;
  - `hashlock`;
  - `expiry` = now + 1200;
  - `outToken` = 0.
- **The note:** USDC `ReceiveWithAuthorization` to SourceLock, valid from now − 60 to now + 300, nonce = `lockIdFor(...)` computed in Rust.
  - The quote lives 120 s, so the lock is sent with more than 15 minutes left and Bob's 15-minute rule holds.
- **Stored:** on the quote (chain, token, amount, lock params, lock id); the hashlock on the order.

### 4b. `POST /v1/orders/:id/legs` (Order Desk)
- Checks that the quote is cross-chain, unexpired, and that the order is `quoted`. Then recovers the signer: it must be the quote's payer.
- Writes the leg (`pending`) and a `source_lock` job in one transaction. Order `quoted → paying`.

### 4c. `POST /v1/orders/:id/secret` (Order Desk)
- Only when the order is `destination_locked`.
- `keccak256(secret)` must equal the order's hashlock.
- Writes a `destination_claim` job with the expected destination lock id, the shop's address and the order amount.

### 4d. Source refunds (Order Desk)
- Every `REFUND_SWEEP_SECONDS` (default 5): for each leg that our Relayer locked, that is not claimed or returned, and whose lock is `REFUND_GRACE_SECONDS` (default 30) past expiry, write a `source_refund` job.
- The grace gives the Matchmaker its turn first; whoever loses the race finds the lock already refunded and sends nothing.
- A failed refund job is written again 2 minutes later, for as long as the lock stays open. A refusal costs no gas, and the Relayer dry-runs before it sends.
- No refund is written while our claim for the order may still be mined: if it lands, the shop is paid and the lock is Bob's.
- The shop's daily gas budget can stop only a payment or a lock, while the money is still in the customer's wallet. A claim or a refund finishes money already locked, so it is always sent.
- When our claim or refund is mined, the Relayer marks the order `paid` or `returned` itself, so the order is right even while the Matchmaker is down.

### 4e. Relayer: several chains, four jobs
- Runs one worker per chain: Whitechain, plus each customer chain whose SourceLock address is set. Each chain has its own nonces, gas budget and RPC (`RPC_<chainId>`, with `SHOP_CHAIN_RPC` kept for Whitechain).

| Job | Call | Checks before sending | "Already done" means |
|---|---|---|---|
| `direct_pay` | `DirectPay.pay` | as today | the note is used; `Paid` log |
| `source_lock` | `SourceLock.lockWithAuthorization` | lock id recomputes; Circle has not blacklisted the payer; balance; ≥ 16 min left on the lock; dry run | the note is used; `Locked` log |
| `destination_claim` | `DestinationLock.claim` | the lock is open, pays this shop at least the order amount, hashlock matches, ≥ 2 min left; Tether has not blocked the DestinationLock; dry run | lock claimed; `Settled` log |
| `source_refund` | `SourceLock.refund` | the lock is open and expired; dry run | lock refunded; `Refunded` log |

- The claim's checks matter: a claim that reverts still puts the secret on chain, where Bob could take the USDC while also getting his USDT back.
- A failed `source_lock` reopens the order (`quoted`) like a failed `direct_pay`. A failed claim or refund leaves the order alone.

### 4f. Events move the order (Order Desk)
- **`Locked`:**
  - must be our leg's exact lock id on that chain, with the same token and amount;
  - leg `locked`; order `source_locked`.
- **`Filled`:**
  - the destination lock id must equal `DestinationLock.lockIdFor(sourceChainId, ourLockId, Bob)` and the amount must cover the order;
  - order `destination_locked`, and the id is stored.
- **`Settled`:**
  - only for a `destination_locked` order, with the filled amount;
  - order `paid`.
- **`Claimed`:** leg `claimed`.
- **`Refunded`:** for our source lock, leg `returned` and the order `returned`, unless already paid.

### 4g. `GET /v1/orders/:id`
- Adds `hashlock`, `legs` (chain, token, amount, lock id, status, lock and claim tx) and `destinationLockId`.
- With these, the pay page can check Bob's lock, and the Matchmaker can require the exact lock id.

### 4h. Migration `0003_crosschain.sql`
- `legs` table.
- `quotes.leg` (the planned lock).
- `orders.destination_lock_id`.
- New job kinds.
- The unfinished-job rule becomes "one per order and kind".

## 5. Open points for A

1. **SPEC:** `/quote` must take the hashlock, because the note's nonce is built from it. `/legs` becomes `{quoteId, signature}`. `/secret` takes `{secret}`. `shared/types.ts` needs `Order.legs` and the quote shape. This needs A's agreement; the code follows this proposal.
2. **`Settled` has no lock id** in `ChainEvent`. Anyone can open a lock on our DestinationLock with an order's id, any shop address and the order's amount, then claim it, and the Matchmaker reports that `Settled` too.
   - Until the event carries the lock id and the shop, the Order Desk accepts `Settled` only for an order whose verified fill it already has, with the same amount. That narrows the hole, but does not close it.
   - Ask A to add `destinationLockId` and `merchant` to `Settled`.
3. **Exact match in the Matchmaker:** once `legs[].lockId` is in `GET /v1/orders/:id`, it can require that id, as its plan says.
4. **Bob's address** for `PROVIDER_ADDRESS`. It is the receiver of the M2 source lock.

## 6. Tasks

| # | Task | Tests | Status |
|---|---|---|---|
| 1 | Jobs and migration | job JSON round trip | ✅ |
| 2 | `lockIdFor`, destination `lockIdFor`, cross-chain quote | unit tests; ids equal the live contracts' (free `eth_call`) | ✅ |
| 3 | `/legs`, `/secret`, refund sweeper, order view | DB tests: wrong signer, expired quote, wrong secret, double submit | ✅ |
| 4 | Events: Locked, Filled, Settled, Claimed, Refunded | DB tests, including wrong lock ids | ✅ |
| 5 | Relayer: chains, four jobs | fake-chain tests for each job | ✅ |
| 6 | Fork run: quote, sign, lock, fill, secret, claim | Base and Whitechain anvil forks with the Matchmaker | ✅ |
| 7 | One live run | real gas, once | to do |

### Fork run (task 6), Fri Nov 6

`server/order-desk/e2e/`: our Order Desk and Relayer with A's Matchmaker, against the deployed contracts on local forks.

- **Paid:** a 1 USDT order. The quote asked 1.003 USDC; the Relayer locked it on Base; Bob filled; `/secret` made the Relayer claim on Whitechain. The shop's USDT went 0 → 1.000000, Bob redeemed the source lock, and the order ended `paid` with its leg `claimed`.
- **Never filled:** a 3 USDT order, over Bob's 2 USDT limit. The Relayer locked 3.009 USDC; the Matchmaker skipped it (`AboveMaxPerOrder`). After the lock timed out, the sweeper queued a refund, the Relayer sent it, and the order ended `returned` with the customer's USDC back in full.
- Seen on the way: the public Base RPC behind the fork answers `eth_getLogs` for at most 200 blocks, so the Matchmaker took about 2 minutes to catch up before filling. Live, it reads only new blocks.

## 7. Funding (testnet)

- **Relayer** `0xD5337E6E2061da3f4C8dA8148F702AD6c47ddA03`: Base Sepolia ETH. It never holds USDC.
- **Test customer** (Privy `0xA917c1caA01032468DeEDb752371f6E27718936d`): a few USDC on Base Sepolia.
- **Test shop:** a payout address that is not the Relayer.
