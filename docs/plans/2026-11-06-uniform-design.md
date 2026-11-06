# Uniform Design

Uniform is a checkout for shops on Whitechain. The shop is always paid in USDT on Whitechain. The customer pays from wherever their money is: USDT on Whitechain, or USDC, EURC or ETH on Base Sepolia or Ethereum Sepolia. The customer logs in by email, signs once and never pays gas.

- Deadline: Sat Nov 7, 09:00 IST (ETHGlobal Mumbai 2026)
- Repo: https://github.com/shaikyajash/Uniform (work on `dev`)
- Interfaces both owners build against: [`docs/SPEC.md`](../SPEC.md) and [`shared/`](../../shared)
- Live progress: the "Uniform Build Plan" page

## 1. People

| Name in the demo | Role | Has | Wants |
|---|---|---|---|
| Kettle & Co | Shop (merchant) | Nothing at the start | USDT on Whitechain |
| Alice, Mei, Ravi | Customers | USDT on Whitechain, or USDC, EURC or ETH on Base or Ethereum Sepolia | To pay with one tap and no gas |
| Bob | Liquidity provider | USDT on Whitechain, shipped to Aqua | A small fee per order (0.3%) |

## 2. Chains

| Chain | Id | Role | Tokens |
|---|---|---|---|
| Whitechain Sepolia | 1874 | Destination chain: the shop is paid here | USDT `0xcedf…55a7` (EIP-712 "Tether USD", version "1") |
| Base Sepolia | 84532 | Source chain | USDC (EIP-712 "USDC", version "2"), EURC, ETH |
| Ethereum Sepolia | 11155111 | Source chain | USDC, EURC, ETH |

Every address lives in `shared/chains.json`.

## 3. Three ways to pay

### Path A: same chain (USDT on Whitechain)

1. The Pay Button asks the customer to sign an EIP-3009 `ReceiveWithAuthorization` whose nonce is `DirectPay.paymentIdFor(orderId, merchant, amount, payer)`.
2. The Gas Payer calls `DirectPay.pay`. USDT moves from the customer to the shop in one transaction, and the contract emits `Paid`.
3. Refunds: the shop signs, the Gas Payer calls `DirectPay.refund`, and the USDT goes back to the original payer (`Refunded`). Partial refunds are allowed up to the paid amount.

### Path B: cross chain (an HTLC, a hash time-locked contract)

```
1. Source lock         Mei → Customer Lock (Base)      12 USDC held for Bob       (Locked)
2. Destination lock    Bob → Shop Lock (Whitechain)    12 USDT held for the shop  (Filled)
3. Destination redeem  Mei reveals the secret          shop gets 12 USDT          (Settled)
4. Source redeem       Bob reuses the public secret    Bob gets 12 USDC           (Claimed)
Timeout: each lock refunds whoever locked it. The destination lock (10 min) expires before the source lock (20 min).
```

- EURC and ETH are swapped to USDC on Uniswap v4 inside the Customer Lock (A6), so Bob always receives USDC.
- Bob's USDT reaches the Shop Lock through 1inch Aqua: the Payout Pool `pull`s it from Bob's shipped strategy.
- Why nobody can cheat: Bob cannot redeem without the secret, and the secret is first used on the Shop Lock, which can only pay the shop. Mei reveals only after her browser has checked the Shop Lock itself.

### Path C: split (two source chains, all or nothing)

The customer locks part on Base and part on Ethereum Sepolia under the same hashlock. The Matchmaker fills only when every leg is locked and the total covers the order. One secret redeems every leg.

## 4. The secret

- `secret = keccak256(signature)`, where the customer's Privy wallet signs EIP-712 data with domain `{name "Uniform", version "1", chainId}` and a message binding `orderId` and `amount`. Signing again gives the same signature, so a closed tab can be recovered.
- `hashlock = keccak256(abi.encodePacked(secret))`.
- The secret and its signature are never stored (no localStorage) and never sent to the server before the reveal.
- The browser reveals only after it has read the Shop Lock and checked the merchant, the amount (at least the order amount), the hashlock and the time left.
- Fallback: a random secret kept only in memory. If the tab closes, the timeouts refund both sides.

## 5. Components

### Contracts (owner A)

| Contract | Chain | Job | Task |
|---|---|---|---|
| Aqua (official 1inch code) | Whitechain | Holds Bob's strategy and lets the Payout Pool pull from it | A1 ✅ `0x02dF…4871` |
| DirectPay | Whitechain | Path A payments and refunds | A2 ✅ `0x9DF1…aE9` |
| CustomerLock | Base, Ethereum Sepolia | Source lock: lock, source redeem, refund, lock for refunds | A3, A6, A7 |
| PayoutPool (AquaApp) | Whitechain | Pulls Bob's USDT into a Shop Lock under Bob's rules | A4 |
| ShopLock | Whitechain | Destination lock: destination redeem (emits `Settled`), refund to Bob | A4, A9 |

### Server (Rust)

One Cargo workspace in `server/`: a binary per service, plus `server/common` (B: shared types and chain config) and `server/bindings` (A: `alloy` contract bindings). HTTP with `axum`, SQLite with `sqlx`, chains with `alloy`. See SPEC.md → Stack.


| Service | Owner | Job | Never | Keys |
|---|---|---|---|---|
| Order Desk | B | Orders, quotes, signatures in, status from events, dashboard data | Sends transactions; marks paid without an event | None |
| Gas Payer | B | Simulates, sends, retries; caps and daily gas budget | Decides status | A small hot wallet per chain |
| Matchmaker | A | Watches all 3 chains, reports events, fills for Bob, source redeems, refunds expired locks | Talks to shops or customers | Bob's filler wallet |
| Shop Notifier | B | Signed webhooks with retries and a log | Reads the chain | Shop webhook secrets |

### Frontend

| Page | Owner | Job |
|---|---|---|
| Pay Button `/pay/[orderId]` | B | Privy email login, price, sign, secret, Shop Lock check, reveal, live status |
| Shop Dashboard | B | Payments, refunds, payment links, settings, CSV |
| Receipt `/receipt/[orderId]` | B | Public proof with every transaction link |
| Earn Page | A | Bob ships or docks his strategy and sees his fees |
| Widget and Demo Shop | B | The 20-line button a shop adds |

## 6. Data

Rule: **only chain events say money moved.** The databases keep business details and a fast copy of the status.

- Order Desk database (SQLite): `merchants`, `orders`, `quotes`, `legs`, `transactions`, `events` (unique on chain, tx hash and log index), `refunds`, `links`, `webhook_log`, `gas_budget`.
- Matchmaker database (its own SQLite file): `cursors` (last block checked per chain, so it catches up after an RPC outage) and `fills`.
- The Matchmaker talks to the Order Desk only through `POST /internal/events`.
- Statuses live in `server/common` (Rust enum with `can_move`) and in `shared/types.ts` for the web; both lists must match:

```
created → quoted → paying → paid                                   (Path A)
created → quoted → paying → customer_locked → shop_locked → paid   (Paths B and C)
customer_locked or shop_locked → returned    created or quoted → expired    paying → quoted
paid → partially_refunded → refunded
```

## 7. Timeouts (demo values)

| Item | Demo | Real product |
|---|---|---|
| Customer Lock (source) | 20 min | 2 h |
| Shop Lock (destination) | 10 min | 1 h |
| Matchmaker fill cutoff | at least 15 min left on the source lock | at least 90 min |
| Quote | 2 min | 2 min |
| Signed authorization | 10 min (contracts reject more than 1 h) | same |

## 8. Security rules

- Every EIP-3009 nonce commits to all the parameters (DirectPay: order, merchant, amount, payer; CustomerLock: the whole LockParams, payer, token and amount), plus a tag, the chain id and the contract. The submitter cannot change anything, nobody can squat another customer's id, and each signature works once.
- Contracts record state before moving tokens.
- The Gas Payer simulates every transaction, checks `authorizationState` and balances first, and submits only for registered shops and issued quotes.
- Only 65-byte EOA signatures are supported (no ERC-1271 smart wallets).
- Private keys stay in gitignored `.env` files. Deploys use the Foundry keystore (`--account deployer`).

## 9. Task map

| Task | What | Depends on | Status |
|---|---|---|---|
| S1 | Spec freeze and repo | none | ✅ |
| A1 | Aqua on Whitechain | WBT | ✅ |
| B0 | Privy check | S1 | ✅ |
| A2 | DirectPay | S1 | ✅ live pay and refund |
| **A3** | **CustomerLock, USDC only, Base Sepolia** | S1 | **next**, see [the A3 plan](2026-11-06-a3-customer-lock.md) |
| A4 | PayoutPool and ShopLock | A1 | to do |
| A5 | Matchmaker v1 | A3, A4 | to do (M2) |
| A6 | Uniswap swap and pools | A3 | to do |
| A7 | Ethereum Sepolia | A6 | to do (M3) |
| A8 | Split payments | A5, A7 | to do |
| A9 | Refunds, chain side | A5, A6 | to do |
| A10 | Earn Page | A4 | to do |
| A11 | Verify all and docs | all A | to do |
| B1 | Rust workspace and `common` crate first, then the Order Desk | S1 | **next** |
| B2 to B10 | Gas Payer, Pay Button, quotes, notifier, dashboard, widget, hosting, README | see the build plan | to do |

Milestones: M1 Path A in the browser · M2 Path B with USDC from Base · M3 EURC on Base and ETH on Sepolia · M4 feature freeze (Sat 02:00) · M5 submitted (Sat 08:30).

## 10. Decisions

| Decision | Why |
|---|---|
| The shop is always paid in USDT on Whitechain | Whitechain's prize requires USDT `0xcedf…` in the main flow |
| HTLC with Bob, not a bridge | Their prize won't place custodial bridges; Bob's USDT gives instant settlement |
| Our own Aqua deployment on Whitechain | 1inch has not deployed Aqua there (confirm at the booth that it counts) |
| Privy for email login | Reown embedded wallets do not support Whitechain; B0 passed |
| Rust for every server service | Team choice; `alloy` covers RPC, signing and contract bindings |
| Uniswap only where a swap is needed (EURC or ETH to USDC) | Use sponsors only where they fit |
| Secret from a signature, no storage | Recoverable without keeping a secret in the browser |
| Lock ids include payer, token, amount, chain and contract | Lesson from the DirectPay security review: stops squatting and replay |
