# A4 PayoutPool and DestinationLock Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** The destination side of a cross-chain payment on Whitechain. Bob's USDT moves from his wallet, through his Aqua strategy, into a destination lock for the merchant. The secret pays the merchant and emits `Settled`. A timeout sends the USDT back to Bob. Done when a script ships Bob's strategy, fills 1 USDT into a DestinationLock, redeems it to the merchant, and `Settled` shows on the Whitechain explorer.

**Architecture:**
- `PayoutPool` is an Aqua app (`AquaApp`). Bob ships a strategy to it with `Aqua.ship(pool, abi.encode(Strategy), [USDT], [allowance])`, so the strategy hash is `keccak256(abi.encode(Strategy))`. His USDT stays in his wallet.
- Only Bob's chosen `filler` (his Matchmaker) can call `fill`, and only up to `maxPerOrder`. `fill` opens a destination lock, then `Aqua.pull`s the USDT straight from Bob's wallet into it.
- `PayoutPool` deploys its own `DestinationLock` in its constructor, so each knows the other with no admin or setter.
- `DestinationLock.claim(lockId, secret)` (destination redeem) pays the merchant set at fill time and emits `Settled` with the secret, which the Matchmaker reads to redeem on the source chain.
- `DestinationLock.refund(lockId)` after expiry returns the USDT to Bob with `Aqua.push`, which restores his strategy allowance. If Bob has docked the strategy, `push` reverts, so the USDT goes to his wallet with a plain transfer instead.
- One destination lock per source lock and provider: `lockId = keccak256(abi.encode(TAG, chainid, this, srcChainId, srcLockId, maker))`, so Bob can never fill the same customer lock twice, and nobody else's lock can block his. The maker was added after the security review.

**Out of scope (A9):** the refund direction (`openRefund`, where the merchant's USDT is locked for Bob).

**Tech Stack:** Solidity 0.8.30, Foundry, @1inch/aqua 1.0.1 (`AquaApp`, `AquaRouter` in tests), OpenZeppelin 5.4 `SafeERC20`.

**Limits:** destination lock time 5 min to 2 h (always shorter than the source lock's 3 h maximum) · amount above zero, at most `maxPerOrder`, fits in uint96 · merchant is not zero, a lock contract or USDT · hashlock not zero · each source lock filled once.

**Rules for the Matchmaker and Order Desk:**
- Fill only the exact source lock id recomputed from the quote, with at least 15 minutes left on it, and set the destination expiry earlier (demo: 10 minutes).
- Before sending a claim, keep at least 2 minutes before expiry, simulate it, and check that Tether hasn't blocked the destination lock. A reverted claim still exposes the secret. (Tether only blocks senders, so a blocked merchant is still paid.)
- Read the secret from `Settled` and redeem on the source chain at once.

---

### Task 1: Tests first

**Files:** `contracts/test/PayoutPool.t.sol`. Uses a local `AquaRouter` and a mock USDT.

Covers:
- fill moves USDT from Bob's wallet into the lock and lowers his Aqua allowance;
- only the filler can fill;
- the maximum per order;
- the wrong token, and an unshipped or docked strategy;
- the same source lock twice;
- only the pool can open a lock;
- open checks: merchant, hashlock, expiry range, amount;
- claim pays the merchant and emits `Settled` with the secret;
- a wrong secret, a claim at expiry, and a double claim;
- a refund before expiry;
- a refund after expiry restores Bob's Aqua allowance;
- a refund after dock falls back to a plain transfer;
- a fuzzed secret.

Run `forge test --match-contract PayoutPoolTest`. Expected: fails to compile (the contracts don't exist yet).

### Task 2: Contracts

**Files:** `contracts/src/PayoutPool.sol`, `contracts/src/DestinationLock.sol`.

Run `forge test`. Expected: every test passes, old and new.

### Task 3: Fork test against the deployed Aqua and the real Whitechain USDT

**Files:** `contracts/test/PayoutPool.fork.t.sol`. Uses Aqua `0x02dF2659B5B4956EAFEe3293584C2aaefC114871` and USDT `0xcedf…55a7`. Nothing is sent.

Expected: ship, fill and claim work, and a refund pushes back into the real Aqua.

### Task 4: Scripts

**Files:**
- `contracts/script/DeployPayoutPool.s.sol`: deploys the pool, which deploys the lock.
- `contracts/script/TryPayoutPool.s.sol`: Bob approves Aqua and ships, the filler fills 1 USDT, the gas payer redeems with the secret.

Simulate the deploy with no `--broadcast`.

### Task 5: Independent security audit, then fixes

### Task 6: Deploy and verify both contracts on Whitechain (the user runs it with the keystore), then a live run

Bob for the live run is the test customer wallet, which holds 1 USDT on Whitechain. It needs a little WBT for its approve and ship transactions. The filler and redeemer is the gas payer, and the merchant is the test merchant.

### Task 7: Record addresses and ABIs in `shared/`, update the spec and the build plan, then open a PR to `dev`
