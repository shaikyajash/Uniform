# Uniform spec

Shared interfaces both owners build against. Change only by agreement between A (chain side) and B (app side).

## Who owns what

Every folder has exactly one owner.

| Folder | Owner |
|---|---|
| `contracts/`, `server/bindings/`, `server/matchmaker/`, `web/app/earn/` | A |
| `server/Cargo.toml` (lists all 6 members from the start), `server/common/`, `server/order-desk/`, `server/gas-payer/`, `server/notifier/`, `web/` (except `app/earn/`), `demo-shop/` | B |
| `shared/`, `docs/SPEC.md` | Both, by agreement (A writes `abis/` and contract addresses, B writes `types.ts`) |

## Stack

| Part | Language | Main libraries |
|---|---|---|
| `contracts/` | Solidity 0.8.30 | Foundry, OpenZeppelin 5.4, 1inch Aqua |
| `server/` (Order Desk, Gas Payer, Shop Notifier, Matchmaker) | **Rust** (stable), one Cargo workspace | `tokio`, `axum` (HTTP), `sqlx` (SQLite, migrations), `alloy` (RPC, `sol!` bindings from `shared/abis`, signing, EIP-712), `serde`, `reqwest` (webhooks, internal events), `hmac` + `sha2` (webhook signature), `tracing` |
| `web/`, `demo-shop/` | TypeScript | Next.js, Privy, viem |

Server layout: `server/Cargo.toml` is a workspace with one binary crate per service (`order-desk`, `gas-payer`, `notifier`, `matchmaker`) plus two library crates: `server/common` (B: status enums and `can_move`, `ChainEvent`, the `chains.json` loader, config) and `server/bindings` (A: `alloy::sol!` contract bindings from `shared/abis/`). `shared/types.ts` stays as the web's copy of the same types, and the two must match.

## Chains and tokens

| Chain | Id | Role | Tokens |
|---|---|---|---|
| Whitechain Sepolia | 1874 | Shop is paid here | USDT `0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7` (6 decimals), gas in WBT |
| Base Sepolia | 84532 | Customer chain | USDC `0x036CbD53842c5426634e7929541eC2318f3dCF7e`, EURC `0x808456652fdb597867f38412077A9182bf77359F`, ETH |
| Ethereum Sepolia | 11155111 | Customer chain | USDC `0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238`, EURC `0x08210F9170F89Ab7658F0B5E3fF39b0E03C594D4`, ETH |

All addresses live in `shared/chains.json`.

## Secret and lock

```
secret   = keccak256(signature)   // customer payments, see below
hashlock = keccak256(abi.encodePacked(secret))
orderId  = bytes32 made by the Order Desk
```

Customer secret (Pay Button, Paths B and C):
- The Privy wallet signs EIP-712 typed data: domain `{ name: "Uniform", version: "1", chainId }`, message binding `orderId` and `amount`. `secret = keccak256(signature)`.
- The same order gives the same signature, so a closed tab is recovered by signing again. Nothing is stored: no localStorage, no server copy, no logs. The signature never leaves the browser.
- Before revealing the secret, the browser reads the Destination Lock on Whitechain itself and checks the merchant, amount (at least the order amount), hashlock and time left. It does not rely on the Order Desk status.
- Privy allowed domains: only our domain plus localhost. No third-party scripts on the pay page.
- Fallback: a random 32-byte secret held only in memory. If the tab closes, the HTLC timeouts refund both sides.

## Cross-chain terms

```
1. Source lock         customer → Source Lock (Base or Ethereum Sepolia)   USDC held for Bob
2. Destination lock    Bob      → Destination Lock (Whitechain)                      USDT held for the shop
3. Destination redeem  customer's browser reveals the secret (via Gas Payer)  shop gets USDT  (Settled)
4. Source redeem       Matchmaker reuses the now-public secret                Bob gets USDC   (Claimed)
Refund (timeout): each lock returns to whoever locked it; the destination lock expires first.
```

The customer only reveals the secret. The Destination Lock pays only the merchant set when it was opened, so the sender of the secret never receives the USDT.

## Timeouts (demo values)

| Lock | Demo | Real product | Rule |
|---|---|---|---|
| Source Lock | 20 min | 2 h | Always the longest |
| Destination Lock | 10 min | 1 h | Always shorter than the customer side |
| Matchmaker fill cutoff | 15 min left | 90 min left | Never fill a source lock close to expiry |
| Quote | 2 min | 2 min | Price and minimum USDC out |

## Order statuses

Defined in `shared/types.ts` (`ORDER_STATUSES`, `ORDER_TRANSITIONS`, `canMove`) for the web, and mirrored in `server/common` as a Rust enum with `can_move` for the server; keep both lists identical. The Order Desk rejects any move not in the table.

```
created → quoted → paying → paid                                   (Path A)
created → quoted → paying → source_locked → destination_locked → paid   (Paths B and C)
source_locked or destination_locked → returned     (timed out, customer's money went back)
created or quoted → expired                   (quote ran out, no money moved)
paying → quoted                               (transaction failed, customer can sign again)
paid → partially_refunded → refunded
```

| Status | Means | Set by |
|---|---|---|
| `created` | Shop made the order | `POST /v1/orders` |
| `quoted` | Customer got a price and the data to sign | `POST /v1/orders/:id/quote` |
| `paying` | Signature received, transaction sent | Gas Payer sent it |
| `source_locked` | Every customer leg is locked | `Locked` events |
| `destination_locked` | Bob's USDT is locked for the shop | `Filled` event |
| `paid` | The shop has the USDT | `Paid` or `Settled` event |
| `partially_refunded` | Part of the amount given back | refund events |
| `refunded` | Full amount given back | refund events |
| `expired` | Quote ran out, no money moved | Order Desk timer |
| `returned` | Cross-chain timed out, customer refunded | Source Lock `Refunded` event |

Legs: `pending → locked → claimed`, or `locked → returned`. Refunds: `pending → sent`, or `pending → failed`.

## Fees

The provider (Bob) charges 0.3% (`providerFeeBps = 30`), added to the customer's quote.

## Contracts (owner A)

### DirectPay (Whitechain) — built

```solidity
function paymentIdFor(bytes32 orderId, address merchant, uint256 amount, address payer) view returns (bytes32);
// = keccak256(abi.encode(PAY_TAG, chainid, this, orderId, merchant, amount, payer)); also the EIP-3009 nonce
function pay(bytes32 orderId, address merchant, uint256 amount, address payer,
             uint256 validAfter, uint256 validBefore, uint8 v, bytes32 r, bytes32 s) returns (bytes32 paymentId);
function refundNonceFor(bytes32 paymentId, uint256 amount, uint256 refundedBefore) view returns (bytes32);
function refund(bytes32 paymentId, uint256 amount, uint256 validAfter, uint256 validBefore,
                uint8 v, bytes32 r, bytes32 s);   // signed by the merchant, paid to the original payer
event Paid(bytes32 indexed paymentId, bytes32 indexed orderId, address indexed merchant, address payer, uint256 amount);
event Refunded(bytes32 indexed paymentId, bytes32 indexed orderId, address indexed payer, uint256 amount, uint256 totalRefunded);
// MAX_AUTH_WINDOW = 1 hour: validBefore must be <= now + 1 hour
```

What the customer (or shop, for refunds) signs with `eth_signTypedData_v4`:

```
domain  = { name: "Tether USD", version: "1", chainId: 1874,
            verifyingContract: 0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7 }
types   = ReceiveWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)
message = { from: customer, to: DirectPay, value: amount, validAfter: now - 60,
            validBefore: now + 600, nonce: paymentIdFor(orderId, merchant, amount, customer) }
```

Rules for the Order Desk and Gas Payer (from the security review):
- Treat an order as paid only from a `Paid` log emitted by the DirectPay address, after the block is final, and read `payments(paymentId)` as the source of truth.
- Check that `payer`, `merchant` and `amount` match the quote. A second or late `Paid` for one orderId means a refund is owed.
- Keep `validBefore` short (10 minutes); the contract rejects more than 1 hour.
- Before sending, simulate with `eth_call` and check `authorizationState(payer, nonce)` and the payer's balance, so cancelled or front-run authorizations don't waste gas.
- Check Tether's `isBlocked` for the merchant and payer.
- Refund signatures depend on the amount refunded so far, so sign one partial refund at a time.
- Known limitation: only 65-byte EOA signatures (no ERC-1271 smart wallets).

### SourceLock (Base Sepolia, Ethereum Sepolia) — USDC and EURC

The lock holds the token the customer paid with and never swaps. Bob converts what he collects to USDC separately, later (one Uniswap batch, many tokens to one).

```solidity
constructor(IERC20 usdc, IERC20 altToken);    // altToken = EURC; both fixed at deploy
function isAllowed(address token) view returns (bool);   // USDC or EURC
struct LockParams { bytes32 orderId; address receiver; uint256 minUsdcOut;
                    bytes32 hashlock; uint64 expiry; address outToken; }
// minUsdcOut: the quote's USDC value; enforced on chain for USDC locks only. For EURC the
// Matchmaker checks the value before filling.

function lockIdFor(LockParams p, address payer, address tokenIn, uint256 amountIn) view returns (bytes32);
// = keccak256(abi.encode(LOCK_TAG, chainid, this, p, payer, tokenIn, amountIn)); also the EIP-3009 nonce
// LOCK_TAG = keccak256("uniform.sourcelock.lock.v2")
function lockWithAuthorization(LockParams p, address payer, address tokenIn, uint256 amountIn,
        uint256 validAfter, uint256 validBefore, uint8 v, bytes32 r, bytes32 s,
        bytes swapData) external returns (bytes32 lockId);
// gasless, USDC or EURC; swapData must be empty
function lock(LockParams p, address token, uint256 amount) external returns (bytes32 lockId);
// the caller is the payer and locks its own approved tokens (provider refund locks; customer
// accounts paying directly, later through EIP-7702)
function claim(bytes32 lockId, bytes32 secret) external;  // source redeem: before expiry, receiver gets the locked token
function refund(bytes32 lockId) external;                 // at or after expiry, the exact token and amount back to the payer
function locks(bytes32) view returns (address payer, uint64 expiry, uint8 state, address receiver,
        uint96 amount, bytes32 hashlock, bytes32 orderId, address token);
event Locked(bytes32 indexed lockId, bytes32 indexed orderId, address payer, address receiver,
             address token, uint256 amount, bytes32 hashlock, uint64 expiry);
event Claimed(bytes32 indexed lockId, bytes32 secret);
event Refunded(bytes32 indexed lockId);
```

Contract limits: lock time 5 min to 3 h · `validBefore - validAfter <= 1 h` · `validBefore + 15 min <= expiry` · amount above zero and fits in uint96 · token is USDC or EURC · receiver is not zero, the contract, USDC or EURC · hashlock not zero · `outToken` 0 or USDC · `swapData` empty · each lock id once · `amount >= minUsdcOut` for USDC locks.

What the customer signs (Base Sepolia shown):

```
USDC domain = { name: "USDC", version: "2", chainId: 84532,
                verifyingContract: 0x036CbD53842c5426634e7929541eC2318f3dCF7e }
EURC domain = { name: "EURC", version: "2", chainId: 84532,
                verifyingContract: 0x808456652fdb597867f38412077A9182bf77359F }
types   = ReceiveWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)
message = { from: customer, to: SourceLock, value: amountIn, validAfter: now - 60,
            validBefore: now + 300, nonce: lockIdFor(params, customer, token, amountIn) }
params.expiry = now + 1200 (demo), so validBefore + 15 min <= expiry holds
```

Rules for the Matchmaker and Gas Payer (from the security reviews):
- **Trust only the exact lock you expect.** Anyone can open a lock with any orderId, receiver or hashlock, using their own USDC or EURC. Recompute `lockIdFor` from the Order Desk's quote and the customer, and accept only that lock id, emitted by our SourceLock address. Never match on orderId or hashlock alone. Then read `locks(lockId)` and check token, amount, receiver, hashlock and expiry.
- **Value an EURC lock before filling:** the locked EURC must be worth at least the order plus Bob's fee at the current EUR/USD price (with a small safety buffer). If the price is unavailable or too low, do not fill; the customer gets the same EURC back at expiry. The Matchmaker reads the price from CoinGecko (`euro-coin` in USD, cached 60 s, refused if older than 10 min or outside $0.90–$1.40) and values EURC at the price less `EURC_PRICE_BUFFER_BPS` (default 100 = 1%), rounded down. A quote for EURC must ask for at least `ceil(required USDC × 10^10 / (price in micro-dollars × (10000 − buffer)))` EURC units, with the same price source and buffer, or Bob will not fill it.
- Fill only with at least 15 minutes left on the source lock, and set the destination lock to expire before it.
- Trust a source lock only after its chain's wait: `confirmations` from chains.json (demo defaults: Base Sepolia 10 blocks, about 2 s; Ethereum Sepolia 3, about 36 s), overridable per chain with `CONFIRMATIONS_<chainId>`. For production, read rollups up to their `safe` block (`SAFE_TAG_CHAINS=84532`) and raise Ethereum to 12 or more; never use `safe` on Ethereum, which trails by about 9 minutes, too long for a 20-minute lock. With the fast defaults a reorg can cost Bob up to his per-order limit. Keep enough gas on each source chain to redeem (Matchmaker: `MIN_FILLER_GAS_WEI_<chainId>`, default 0.002 ETH on Ethereum). See README.md.
- Before filling, check that Circle has not blacklisted Bob for that token (`isBlacklisted`); a blacklisted receiver cannot claim, and the customer would be refunded after Bob has paid.
- Claim as soon as the secret appears on Whitechain; do not wait near expiry.
- Known limit: if Circle blacklists the customer after locking, refund reverts and the locked token stays in the contract.

### PayoutPool (Whitechain, AquaApp) — built

```solidity
struct Strategy { address maker; address token; address filler; uint128 maxPerOrder;
                  uint16 feeBps; bytes32 salt; }
// Bob ships it: Aqua.ship(payoutPool, abi.encode(strategy), [USDT], [allowance]);
// strategyHash = keccak256(abi.encode(strategy)). feeBps is read off-chain when quoting.
struct Fill { bytes32 orderId; address merchant; uint256 amount; bytes32 hashlock;
              uint64 expiry; uint256 srcChainId; bytes32 srcLockId; }
function fill(Strategy s, Fill f) external returns (bytes32 lockId);
// only s.filler, s.token == USDT, amount <= s.maxPerOrder;
// opens the DestinationLock, then AQUA.pull(maker, strategyHash, USDT, amount, destinationLock)
function strategyHashOf(Strategy s) pure returns (bytes32);
function destinationLock() view returns (DestinationLock);   // deployed by the pool's constructor
event Filled(bytes32 indexed orderId, address indexed maker, bytes32 indexed strategyHash,
             uint256 amount, bytes32 lockId);
```

### DestinationLock (Whitechain) — built (payout direction; `openRefund` comes in A9)

```solidity
function lockIdFor(uint256 srcChainId, bytes32 srcLockId, address maker) view returns (bytes32);
function open(OpenParams p) external returns (bytes32 lockId);   // only the PayoutPool
function claim(bytes32 lockId, bytes32 secret) external;         // destination redeem: USDT to merchant
function refund(bytes32 lockId) external;                         // at or after expiry, anyone:
// aqua.push back into Bob's strategy (allowance restored); if that reverts (docked), plain transfer to Bob
event Opened(bytes32 indexed lockId, bytes32 indexed orderId, address indexed merchant, address maker,
             uint256 amount, bytes32 hashlock, uint64 expiry, uint256 srcChainId, bytes32 srcLockId,
             bytes32 strategyHash);
event Settled(bytes32 indexed lockId, bytes32 indexed orderId, address indexed merchant, uint256 amount,
              uint256 srcChainId, bytes32 srcLockId, bytes32 secret);   // the receipt; reveals the secret
event Refunded(bytes32 indexed lockId, address indexed maker, uint256 amount, bool backToAqua);
```

Contract limits: lock time 5 min to 2 h · amount above zero, at most `maxPerOrder` and uint96 · merchant is not zero, the lock, the pool or USDT · hashlock not zero · each (source lock, maker) filled once.

Rules from the security review:
- **Watchers trust only our contracts and exact ids.** Accept events only from `payoutPool.destinationLock()`, and only the lock id recomputed with `lockIdFor(srcChainId, srcLockId, Bob)`. Anyone can ship a strategy and open locks of their own.
- **Before revealing the secret** (Pay Button, B5): read `locks(lockId)` and check maker = the source lock's receiver, merchant, amount at least the order amount, hashlock, and an expiry earlier than the source lock's with time to spare.
- **Before sending a claim** (Gas Payer): keep a margin of at least 2 minutes before `expiry`, simulate with `eth_call`, and check Tether's `isBlocked(destinationLock)`. A claim that reverts still exposes the secret in its calldata, so Bob could redeem the source lock while also being refunded here.
- Tether's blocklist checks the sender, not the recipient. A blocked merchant is still paid (the funds are frozen in its wallet), so checking `isBlocked(merchant)` before quoting is a courtesy, not a safety rule.
- **The filler key can spend Bob's whole shipped allowance** (it chooses the merchant and hashlock). Ship a small allowance, refill it as needed, and `dock` the strategy to stop everything at once.

## Order Desk API (owner B)

| Call | Caller | Does |
|---|---|---|
| `POST /v1/orders` | Shop server (API key) | `{amount, reference, successUrl}` → `{orderId, payUrl}` |
| `POST /v1/links` | Shop Dashboard | Payment link with amount, memo, due date |
| `GET /v1/orders/:id` | Pay Button, receipt page | Status, legs, transaction links |
| `POST /v1/orders/:id/quote` | Pay Button | `{payer, balances}` → path, legs, EIP-712 data, expiry |
| `POST /v1/orders/:id/direct` | Pay Button | Signature → Gas Payer calls `DirectPay.pay` |
| `POST /v1/orders/:id/legs` | Pay Button | `{hashlock, legs:[{chainId, signature}]}` → `lockWithAuthorization` |
| `POST /v1/orders/:id/secret` | Pay Button | Only when `destination_locked`: Gas Payer calls `DestinationLock.claim` |
| `POST /v1/orders/:id/refunds` | Shop Dashboard | `{amount, hashlock, signature}` → refund flow |
| `POST /v1/refunds/:id/secret` | Shop Dashboard | Releases the refund to the customer |
| `GET /v1/payments.csv` | Shop Dashboard | Accountant export |
| `POST /internal/events` | Matchmaker | Chain events (`ChainEvent` in `server/common`, mirrored in `shared/types.ts`) |

### Webhook

```
POST {shop webhook url}
X-Uniform-Signature: hex(HMAC_SHA256(body, shop_secret))
{ "type": "order.paid" | "order.refunded" | "order.expired",
  "orderId": "0x…", "reference": "shop-123", "amount": "12.000000", "currency": "USDT",
  "path": "direct" | "crosschain" | "split",
  "receipts": { "customer": ["…tx url"], "settlement": "…tx url" } }
```

### Gas Payer limits

- Only submits for orders from registered shops, with signatures matching the issued quote.
- Simulates every transaction first.
- Caps on order size and a gas budget per shop per day.
- Separate hot wallet per chain with a small balance.
