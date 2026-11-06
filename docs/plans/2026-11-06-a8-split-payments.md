# A8: split payments (Path C)

One order, paid from two chains at once: part of the USDC locked on Base Sepolia, the rest on
Ethereum Sepolia, both for Bob and under one hashlock. Bob fills the shop's destination lock only
once every lock is in and together they cover the order plus his fee. One secret then pays the shop
and lets Bob redeem every lock. If any lock is missing, Bob never fills, and every lock that did
open is refunded after its expiry.

Example: a 10 USDT order with a 0.3% fee needs 10.03 USDC. The customer has 6 USDC on Base and
5 on Ethereum, so they lock 6 on Base and 4.03 on Ethereum.

## API (pay page, B5)

`POST /v1/orders/{id}/quote`

```json
{ "payer": "0x…", "hashlock": "0x…",
  "legs": [ { "chainId": 84532, "amount": "6" }, { "chainId": 11155111 } ] }
```

- `amount` is USDC as a decimal string. One leg may leave it out and gets the rest.
- Rules: at least two legs, each on a different chain that has a SourceLock, each at least
  0.1 USDC, and the amounts add up to exactly `total` (the order plus Bob's fee, rounded up).
  Otherwise the reply is `400 bad_legs` (or `bad_chain`).
- `chainId` and `legs` cannot both be sent.
- Reply: as for one chain, but with `"path": "split"` and one entry in `toSign` per leg. Each
  entry has its own `chainId`, `amount`, `lockId` and `typedData`, and is signed on its own chain.
  All the locks share one hashlock and one expiry.

`POST /v1/orders/{id}/legs`

```json
{ "quoteId": "q_…", "signatures": ["0x…", "0x…"] }
```

- One signature per `toSign` entry, in the same order. A single-chain quote may still send
  `"signature"`.
- Reply: `{status, jobId, lockId, locks: [{chainId, lockId, jobId}]}`. `jobId` is null for a lock
  whose job is already running or done.
- All or nothing: if one lock's job fails while another lock is open or being sent, the order stays
  `paying`. It becomes `returned` once the open lock is refunded (after its expiry). Only when no
  lock opened at all does the order go back to `quoted`, so the customer can sign again. Signing
  the same quote again never sends a lock twice.

### One click on the pay page

The customer clicks **Pay** once. Behind that click the page asks Privy for every signature it needs
(the secret's signature, then one note per leg) with `uiOptions: { showWalletUIs: false }`, so no
Privy window opens for them (https://docs.privy.io/wallets/using-wallets/ethereum/sign-typed-data).
The page's own screen says what is being paid: the total and how much comes from each chain.
Privy's docs do not say whether an embedded wallet signs typed data for a chain other than its
current one without `switchChain`. Embedded wallets sign locally, so it is expected to work. Check
it once in the pay page; if it does not, call `switchChain` before each note, still without a
window.

`GET /v1/orders/{id}` lists every leg in `legs`. The order stays `paying` until every leg is
locked, then moves as usual: `source_locked → destination_locked → paid`. The pay page checks the
destination lock as before. Its `srcChainId` and `srcLockId` name the first leg, sorted by
(chainId, lockId).

## What each part does

- **Matchmaker:**
  - Reads `legs[].lockId` from the Order Desk, and fills only those locks. An order that lists no
    signed locks yet gets no fill: anyone can lock with an order's id.
  - Remembers every Locked log it has read at confirmation depth (`seen_locks`).
  - Fills once all legs are seen, if `rules::combine` passes: every lock open, for Bob, under one
    hashlock, worth enough in total, and the shortest time left at least the fill cutoff.
  - Redeems each leg with the secret (`fill_legs`), and refunds per leg any lock it could not redeem.
- **Order Desk:**
  - Quotes and stores one planned leg per chain.
  - Writes one `source_lock` job per leg.
  - The refund sweep covers every open lock of an unpaid order, and locks from an earlier quote of
    a paid order.
  - The order becomes `returned` only once no lock is still open.
- **Gas Payer:** a failed lock hands the order back to `quoted` only when no other lock of it is
  open or being sent, and a refund makes the order `returned` only once no lock is still open
  (locked, or mined but not yet reported). Both lock the order row first, so two jobs of one split
  finishing at once cannot both miss the other. The order limit applies to the whole order, not to
  each lock.
  Migration 0004 allows one live job per order, kind and chain.

## Tests

- Unit tests: Matchmaker 45, Order Desk 66, Gas Payer 34.
- Fork test (`server/order-desk/e2e/pay.sh <amount> <label> split|split-missing`):
  - split: 1 USDC on Base plus the rest on Ethereum pays the shop, and Bob redeems both.
  - missing leg: Bob does not fill, the Base lock is refunded in full after expiry, and the order
    ends `returned`.

## Known limits

- The lock expiry is set when quoting. A slow signer on two chains eats into the 15 minutes Bob
  needs, and with too little time left he does not fill. The locks are then refunded.
