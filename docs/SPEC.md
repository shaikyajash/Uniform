# Uniform spec

Shared interfaces both owners build against. Change only by agreement between A (chain side) and B (app side).

## Chains and tokens

| Chain | Id | Role | Tokens |
|---|---|---|---|
| Whitechain Sepolia | 1874 | Shop is paid here | USDT `0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7` (6 decimals), gas in WBT |
| Base Sepolia | 84532 | Customer chain | USDC `0x036CbD53842c5426634e7929541eC2318f3dCF7e`, EURC `0x808456652fdb597867f38412077A9182bf77359F`, ETH |
| Ethereum Sepolia | 11155111 | Customer chain | USDC `0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238`, EURC `0x08210F9170F89Ab7658F0B5E3fF39b0E03C594D4`, ETH |

All addresses live in `shared/chains.json`.

## Secret and lock

```
secret   = 32 random bytes, made in the browser (Pay Button or Shop Dashboard)
hashlock = keccak256(abi.encodePacked(secret))
orderId  = bytes32 made by the Order Desk
```

## Timeouts (demo values)

| Lock | Demo | Real product | Rule |
|---|---|---|---|
| Customer Lock | 20 min | 2 h | Always the longest |
| Shop Lock | 10 min | 1 h | Always shorter than the customer side |
| Matchmaker fill cutoff | 15 min left | 90 min left | Never fill a customer lock close to expiry |
| Quote | 2 min | 2 min | Price and minimum USDC out |

## Order statuses

```
created → quoted → customer_locked → shop_locked → paid
                                             ↘ expired (customer refunded)
paid → partially_refunded → refunded
Direct path: created → paid
```

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

### CustomerLock (Base Sepolia, Ethereum Sepolia)

```solidity
struct LockParams { bytes32 orderId; address receiver; uint256 minUsdcOut;
                    bytes32 hashlock; uint64 expiry; address outToken; }

function lockWithAuthorization(LockParams p, address payer, address tokenIn, uint256 amountIn,
        uint256 validAfter, uint256 validBefore, uint8 v, bytes32 r, bytes32 s,
        bytes swapData) external returns (bytes32 lockId);
// tokenIn is USDC or EURC (EIP-3009). Nonce = keccak256(abi.encode(p)), so the relayer
// cannot change receiver, hashlock or expiry. Non-USDC input is swapped on Uniswap v4,
// require(out >= minUsdcOut).
function lockWithETH(LockParams p, bytes swapData) external payable returns (bytes32 lockId);
function lockFromProvider(LockParams p, uint256 usdcAmount) external returns (bytes32 lockId);
function claim(bytes32 lockId, bytes32 secret) external;  // to receiver, swapped to outToken if set
function refund(bytes32 lockId) external;                 // after expiry, back to payer
event Locked(bytes32 indexed lockId, bytes32 indexed orderId, address payer, address receiver,
             uint256 usdcAmount, bytes32 hashlock, uint64 expiry);
event Claimed(bytes32 indexed lockId, bytes32 secret);
event Refunded(bytes32 indexed lockId);
```

### PayoutPool (Whitechain, AquaApp)

```solidity
struct Strategy { address maker; address token; uint16 feeBps; uint128 maxPerOrder;
                  address filler; bytes32 salt; }
function fill(Strategy s, bytes32 orderId, address merchant, uint256 amount, bytes32 hashlock,
              uint64 expiry, uint256 srcChainId, bytes32 srcLockId) external returns (bytes32 shopLockId);
// only s.filler, amount <= s.maxPerOrder,
// AQUA.pull(s.maker, keccak256(abi.encode(s)), USDT, amount, shopLock), then shopLock.openFromPool(...)
event Filled(bytes32 indexed orderId, address indexed maker, uint256 amount, bytes32 shopLockId);
```

### ShopLock (Whitechain)

```solidity
function openFromPool(...) external;           // only PayoutPool
function claim(bytes32 lockId, bytes32 secret) external;
// payout lock: USDT to merchant, emits Settled (onchain receipt)
// refund lock: aqua.push(maker, payoutPool, strategyHash, USDT, amount) back to the provider
function refund(bytes32 lockId) external;       // after expiry
function openRefund(bytes32 orderId, address merchant, uint256 amount, bytes32 hashlock, uint64 expiry,
        address maker, bytes32 strategyHash, uint256 validAfter, uint256 validBefore,
        uint8 v, bytes32 r, bytes32 s) external;
event Settled(bytes32 indexed orderId, address merchant, uint256 amount, uint256 srcChainId, bytes32 srcLockId);
event RefundLocked(bytes32 indexed orderId, bytes32 lockId, uint256 amount);
```

If the provider has docked the strategy, `aqua.push` reverts and ShopLock falls back to a plain transfer.

## Order Desk API (owner B)

| Call | Caller | Does |
|---|---|---|
| `POST /v1/orders` | Shop server (API key) | `{amount, reference, successUrl}` → `{orderId, payUrl}` |
| `POST /v1/links` | Shop Dashboard | Payment link with amount, memo, due date |
| `GET /v1/orders/:id` | Pay Button, receipt page | Status, legs, transaction links |
| `POST /v1/orders/:id/quote` | Pay Button | `{payer, balances}` → path, legs, EIP-712 data, expiry |
| `POST /v1/orders/:id/direct` | Pay Button | Signature → Gas Payer calls `DirectPay.pay` |
| `POST /v1/orders/:id/legs` | Pay Button | `{hashlock, legs:[{chainId, signature}]}` → `lockWithAuthorization` |
| `POST /v1/orders/:id/secret` | Pay Button | Only when `shop_locked`: Gas Payer calls `ShopLock.claim` |
| `POST /v1/orders/:id/refunds` | Shop Dashboard | `{amount, hashlock, signature}` → refund flow |
| `POST /v1/refunds/:id/secret` | Shop Dashboard | Releases the refund to the customer |
| `GET /v1/payments.csv` | Shop Dashboard | Accountant export |
| `POST /internal/events` | Matchmaker | Chain events (`ChainEvent` in `shared/types.ts`) |

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
