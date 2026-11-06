# To do

What is left, in one place. A is chain side (contracts, Matchmaker, Earn page); B is app side
(Order Desk, Gas Payer, notifier, web, demo store). Tick an item in the PR that does it.

Deadline: Sat Nov 7, 09:00 IST. Feature freeze: Sat 02:00.

## Next up
- [ ] **Merge PR #6 (B4 cross-chain).** It now has A's four fixes. Then one live run on real gas. (B)
- [ ] **The pay page's cross-chain flow (B5)** must send `chainId` (84532 or 11155111) in `POST /quote`. Both chains are open now, so a quote without it is refused. (B)
- [ ] **Pitch and go-to-market,** with the 3-minute script. This is 60% of the judging. (A + B)
- [x] **A8: split payments** (part on Base, part on Ethereum Sepolia; see the A8 plan). The pay
  page (B5) must send `legs` in `POST /quote` and `signatures` in `POST /legs`, and sign every
  note behind one **Pay** click with Privy's `showWalletUIs: false` (see the A8 plan). (A + B)
- [ ] **A9: merchant refunds** on the cross-chain path. (A)
- [ ] **A10: Earn page.** (A)

## Safety and correctness
- [ ] **`Settled` must say which lock and which merchant.** Add `destinationLockId` and `merchant`, so the
  Order Desk can tell a real payment from a fake one. Today, anyone can open a lock with an order's
  id and claim it, and the Matchmaker reports that `Settled` too. (A, Matchmaker and `common`)
- [x] **The Matchmaker should fill only our exact lock** (A8: it fills only locks in the order's
  `legs`, and nothing for an order with none signed yet). It should read `legs[].lockId` from
  `GET /v1/orders/{id}` and fill that lock only. Today, a stranger's lock with a known order id can
  take the fill. (A)
- [ ] **Claim deadline: use Bob's 10 minutes, not the customer's 20.** `secret.rs` gives the claim
  job the source lock's expiry. Bob's lock ends 10 minutes earlier, so a stuck claim keeps being
  re-sent with higher fees after it can no longer work. No money is lost: a little gas is wasted,
  and the refund comes about 9 minutes late. Fix: use the destination lock's expiry. (B)
- [ ] **If the Matchmaker dies right after Bob fills, the payment fails.**
  - What happens: Bob's lock is on chain, but the Order Desk never heard about it, so it refuses
    the secret.
  - The customer is refunded after 20 minutes, and Bob gets his USDT back when his Matchmaker
    returns. No money is lost; only the payment fails.
  - Fix: `/secret` works out Bob's lock id itself, as `filled()` does, for a `source_locked`
    order. The Gas Payer already checks the lock on chain before claiming. (B)
- [x] **Split orders: refund every lock** (A8). When an order has two locks, the first refund marks the
  whole order `returned`, and the sweeper then skips the second lock. Move the order only when no
  lock is still open. Not reachable until A8. (B, with A8)
- [ ] **Split: set the lock expiry at `/legs`, not at quote time,** so slow signing on two chains
  does not eat Bob's 15 minutes. (B)
- [ ] **Limit failed payments per wallet.** A user can make us pay gas for a lock plus a refund,
  over and over, with orders Bob will not fill; a split makes it easier (empty one chain before the
  Gas Payer sends). The merchant's daily gas budget caps this at about twice the budget per merchant per
  day. To make it tighter, limit failed cross-chain payments per wallet per hour. (B)
- [ ] **Webhooks must watch the order's status.** `paid` is written in two places: by the Gas Payer
  when its claim is mined, and by `Settled`. So the notifier must react to the order's status, not
  to one of the two events. (B, notifier)
- [ ] **Re-send stuck Ethereum transactions with a higher fee** (Matchmaker). (A)
- [ ] **Check each contract's code hash at start-up,** not only that code exists (Matchmaker). (A)

## Later
- [ ] **EURC/USDC Uniswap v4 pool.** Bob converts the EURC he collects to USDC, and the price check
  moves to the Uniswap Quoter. The script and fork test are written but not committed. (A)
- [ ] **A11: verify the contracts** on the explorers, and tidy the docs. (A)
- [ ] **A12: EIP-7702 "pay with any token"**, only if there is time. (A)

## Testnet funds
- [ ] **Ask DevRel for about 300 USDT and 5 WBT,** for Bob's strategy and the demo.
- [ ] **Top up Gas Payer ETH** before the demo. Today it has about 0.03 ETH on Ethereum Sepolia
  and about 0.005 ETH on Base Sepolia.
