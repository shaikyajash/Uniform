# A3 SourceLock Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** A source-lock contract on Base Sepolia that holds a customer's USDC for Bob under a hashlock and an expiry, locked gaslessly with one EIP-3009 signature. It is done when a script locks 1 USDC and redeems it with the secret on Base Sepolia.

**Architecture:** `SourceLock` takes USDC through `receiveWithAuthorization`, using the lock id as the nonce: `keccak256(abi.encode(LOCK_TAG, chainid, this, LockParams, payer, tokenIn, amountIn))`. The submitter (Gas Payer) therefore cannot change the receiver, hashlock, expiry or amount. `claim(lockId, secret)` before expiry pays the receiver (source redeem, `Claimed`). `refund(lockId)` at or after expiry pays the payer back (`Refunded`). `lockFromProvider` lets Bob lock his own approved USDC for the refund direction (used by A9). Only USDC is accepted for now. `swapData` must be empty and `outToken` must be zero or USDC; A6 adds the Uniswap swap. See the [design](2026-11-06-uniform-design.md), sections 3 and 4.

**Tech Stack:** Solidity 0.8.30, Foundry (via_ir, prague), OpenZeppelin 5.4.0 `SafeERC20`, Circle USDC (FiatTokenV2_2) EIP-3009. Test helpers already in the repo: `test/mocks/MockEIP3009.sol` and `test/utils/Sign3009.sol`.

**Change to SPEC.md:** the spec says the nonce is `keccak256(abi.encode(p))`. This plan also binds payer, token, amount, chain and contract (the same lesson as the DirectPay review), and adds `lockIdFor(p, payer, tokenIn, amountIn)` for the Order Desk to compute it. Task 7 updates the spec.

**Limits enforced by the contract:** lock time 5 min to 3 h · authorization valid for at most 1 h · amount fits in uint96 · non-zero receiver and hashlock · each lock id once.

**After the security review:** the authorization must also end at least 15 minutes before the lock expires, its lifetime is measured from `validAfter`, and the receiver cannot be the contract or USDC. The current contract and its 25 unit tests in `contracts/` are the source of truth; the code below is the first version.

---

### Task 1: Write the failing unit tests

**Files:**
- Test: `contracts/test/SourceLock.t.sol`

**Step 1: Write the test file**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SourceLock} from "../src/SourceLock.sol";
import {MockEIP3009} from "./mocks/MockEIP3009.sol";
import {Sign3009} from "./utils/Sign3009.sol";

contract SourceLockTest is Sign3009 {
    MockEIP3009 usdc;
    SourceLock lock;
    uint256 customerKey = 0xC0FFEE;
    address customer;
    address bob = makeAddr("bob");
    address gasPayer = makeAddr("gasPayer");
    bytes32 secret = keccak256("mei-secret");
    bytes32 hashlock = keccak256(abi.encodePacked(keccak256("mei-secret")));

    function setUp() public {
        vm.warp(1_800_000_000);
        customer = vm.addr(customerKey);
        usdc = new MockEIP3009();
        lock = new SourceLock(IERC20(address(usdc)));
        usdc.mint(customer, 50e6);
    }

    function _params(uint256 minOut) internal view returns (SourceLock.LockParams memory p) {
        p = SourceLock.LockParams({
            orderId: keccak256("order-1002"),
            receiver: bob,
            minUsdcOut: minOut,
            hashlock: hashlock,
            expiry: uint64(block.timestamp + 20 minutes),
            outToken: address(0)
        });
    }

    function _sign(SourceLock.LockParams memory p, uint256 amount) internal view returns (Auth memory) {
        bytes32 nonce = lock.lockIdFor(p, customer, address(usdc), amount);
        return signReceive(
            address(usdc), customerKey, address(lock), amount, block.timestamp - 1, block.timestamp + 10 minutes, nonce
        );
    }

    function _lock(SourceLock.LockParams memory p, uint256 amount, Auth memory a) internal returns (bytes32) {
        vm.prank(gasPayer);
        return
            lock.lockWithAuthorization(
                p, customer, address(usdc), amount, a.validAfter, a.validBefore, a.v, a.r, a.s, ""
            );
    }

    function test_lockThenClaimPaysReceiver() public {
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 id = _lock(p, 12e6, _sign(p, 12e6));
        assertEq(usdc.balanceOf(address(lock)), 12e6);
        assertEq(usdc.balanceOf(customer), 38e6);

        lock.claim(id, secret);
        assertEq(usdc.balanceOf(bob), 12e6);
        assertEq(usdc.balanceOf(address(lock)), 0);
    }

    function test_claimRejectsWrongSecret() public {
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 id = _lock(p, 12e6, _sign(p, 12e6));
        vm.expectRevert(abi.encodeWithSelector(SourceLock.WrongSecret.selector, id));
        lock.claim(id, keccak256("guess"));
    }

    function test_claimTwiceReverts() public {
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 id = _lock(p, 12e6, _sign(p, 12e6));
        lock.claim(id, secret);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.NotOpen.selector, id));
        lock.claim(id, secret);
    }

    function test_claimAtExpiryReverts() public {
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 id = _lock(p, 12e6, _sign(p, 12e6));
        vm.warp(p.expiry);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.LockExpired.selector, id));
        lock.claim(id, secret);
    }

    function test_refundBeforeExpiryReverts() public {
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 id = _lock(p, 12e6, _sign(p, 12e6));
        vm.expectRevert(abi.encodeWithSelector(SourceLock.NotExpired.selector, id));
        lock.refund(id);
    }

    function test_refundAfterExpiryReturnsToPayer() public {
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 id = _lock(p, 12e6, _sign(p, 12e6));
        vm.warp(p.expiry);
        lock.refund(id);
        assertEq(usdc.balanceOf(customer), 50e6);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.NotOpen.selector, id));
        lock.claim(id, secret);
    }

    function test_submitterCannotChangeReceiver() public {
        SourceLock.LockParams memory p = _params(12e6);
        Auth memory a = _sign(p, 12e6);
        p.receiver = gasPayer;
        vm.expectRevert("invalid signature");
        _lock(p, 12e6, a);
    }

    function test_submitterCannotChangeHashlockOrExpiry() public {
        SourceLock.LockParams memory p = _params(12e6);
        Auth memory a = _sign(p, 12e6);
        p.hashlock = keccak256("other");
        vm.expectRevert("invalid signature");
        _lock(p, 12e6, a);

        p = _params(12e6);
        p.expiry += 1;
        vm.expectRevert("invalid signature");
        _lock(p, 12e6, a);
    }

    function test_sameAuthorizationTwiceReverts() public {
        SourceLock.LockParams memory p = _params(12e6);
        Auth memory a = _sign(p, 12e6);
        bytes32 id = _lock(p, 12e6, a);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.LockExists.selector, id));
        _lock(p, 12e6, a);
    }

    function test_rejectsExpiryOutOfRange() public {
        SourceLock.LockParams memory p = _params(12e6);
        p.expiry = uint64(block.timestamp + 1 minutes);
        Auth memory a = _sign(p, 12e6);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.ExpiryOutOfRange.selector, p.expiry));
        _lock(p, 12e6, a);

        p.expiry = uint64(block.timestamp + 4 hours);
        a = _sign(p, 12e6);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.ExpiryOutOfRange.selector, p.expiry));
        _lock(p, 12e6, a);
    }

    function test_rejectsAmountBelowMinimum() public {
        SourceLock.LockParams memory p = _params(12e6);
        Auth memory a = _sign(p, 11e6);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.BelowMinimum.selector, 11e6, 12e6));
        _lock(p, 11e6, a);
    }

    function test_rejectsOtherTokensAndSwapData() public {
        SourceLock.LockParams memory p = _params(12e6);
        Auth memory a = _sign(p, 12e6);
        address eurc = makeAddr("eurc");
        vm.expectRevert(abi.encodeWithSelector(SourceLock.UnsupportedToken.selector, eurc));
        lock.lockWithAuthorization(p, customer, eurc, 12e6, a.validAfter, a.validBefore, a.v, a.r, a.s, "");

        vm.expectRevert(SourceLock.SwapNotSupported.selector);
        lock.lockWithAuthorization(
            p, customer, address(usdc), 12e6, a.validAfter, a.validBefore, a.v, a.r, a.s, hex"01"
        );
    }

    function test_rejectsLongAuthorizationWindow() public {
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 nonce = lock.lockIdFor(p, customer, address(usdc), 12e6);
        Auth memory a = signReceive(
            address(usdc), customerKey, address(lock), 12e6, block.timestamp - 1, block.timestamp + 2 hours, nonce
        );
        vm.expectRevert(
            abi.encodeWithSelector(
                SourceLock.AuthorizationWindowTooLong.selector, a.validBefore, block.timestamp + 1 hours
            )
        );
        _lock(p, 12e6, a);
    }

    function test_lockFromProvider() public {
        usdc.mint(bob, 5e6);
        SourceLock.LockParams memory p = _params(4e6);
        p.receiver = customer;
        vm.startPrank(bob);
        usdc.approve(address(lock), 4e6);
        bytes32 id = lock.lockFromProvider(p, 4e6);
        vm.stopPrank();

        lock.claim(id, secret);
        assertEq(usdc.balanceOf(customer), 54e6);
        assertEq(usdc.balanceOf(bob), 1e6);
    }

    function testFuzz_onlyTheSecretOpensTheLock(bytes32 guess) public {
        vm.assume(guess != secret);
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 id = _lock(p, 12e6, _sign(p, 12e6));
        vm.expectRevert(abi.encodeWithSelector(SourceLock.WrongSecret.selector, id));
        lock.claim(id, guess);
    }
}
```

**Step 2: Run it and check that it fails**

Run: `cd contracts && forge test --match-contract SourceLockTest`
Expected: FAIL to compile with `Source "src/SourceLock.sol" not found`.

---

### Task 2: Write SourceLock

**Files:**
- Create: `contracts/src/SourceLock.sol`

**Step 1: Write the contract**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";

/// @notice EIP-3009 `receiveWithAuthorization`, as implemented by Circle's USDC.
interface IERC3009 {
    function receiveWithAuthorization(
        address from,
        address to,
        uint256 value,
        uint256 validAfter,
        uint256 validBefore,
        bytes32 nonce,
        uint8 v,
        bytes32 r,
        bytes32 s
    ) external;
}

/// @title SourceLock
/// @notice The source lock of a Uniform cross-chain payment. The customer's USDC is held for a
/// receiver (normally Bob, the liquidity provider) under a hashlock and an expiry:
///   - claim(lockId, secret) before expiry pays the receiver (source redeem);
///   - refund(lockId) at or after expiry returns the USDC to the payer.
/// Customers lock gaslessly with an EIP-3009 authorization whose nonce is the lock id:
///     lockId = keccak256(abi.encode(LOCK_TAG, chainid, this, params, payer, tokenIn, amountIn))
/// so a submitter cannot change the receiver, hashlock, expiry or amount, and each authorization
/// works once. Only USDC is accepted for now; swaps from other tokens are added separately.
contract SourceLock {
    using SafeERC20 for IERC20;

    enum State {
        None,
        Open,
        Claimed,
        Refunded
    }

    struct LockParams {
        bytes32 orderId;
        address receiver;
        uint256 minUsdcOut;
        bytes32 hashlock;
        uint64 expiry;
        address outToken;
    }

    struct Lock {
        address payer;
        uint64 expiry;
        State state;
        address receiver;
        uint96 amount;
        bytes32 hashlock;
        bytes32 orderId;
    }

    bytes32 public constant LOCK_TAG = keccak256("uniform.sourcelock.lock.v1");
    uint256 public constant MIN_LOCK_TIME = 5 minutes;
    uint256 public constant MAX_LOCK_TIME = 3 hours;
    uint256 public constant MAX_AUTH_WINDOW = 1 hours;

    IERC20 public immutable usdc;

    mapping(bytes32 lockId => Lock) public locks;

    event Locked(
        bytes32 indexed lockId,
        bytes32 indexed orderId,
        address payer,
        address receiver,
        uint256 usdcAmount,
        bytes32 hashlock,
        uint64 expiry
    );
    event Claimed(bytes32 indexed lockId, bytes32 secret);
    event Refunded(bytes32 indexed lockId);

    error ZeroToken();
    error ZeroAmount();
    error AmountTooLarge(uint256 amount);
    error ZeroReceiver();
    error ZeroHashlock();
    error ExpiryOutOfRange(uint64 expiry);
    error UnsupportedToken(address token);
    error SwapNotSupported();
    error BelowMinimum(uint256 amount, uint256 minUsdcOut);
    error AuthorizationWindowTooLong(uint256 validBefore, uint256 latestAllowed);
    error LockExists(bytes32 lockId);
    error NotOpen(bytes32 lockId);
    error LockExpired(bytes32 lockId);
    error NotExpired(bytes32 lockId);
    error WrongSecret(bytes32 lockId);

    constructor(IERC20 usdc_) {
        if (address(usdc_) == address(0)) revert ZeroToken();
        usdc = usdc_;
    }

    /// @notice The lock id, which is also the EIP-3009 nonce the customer signs.
    function lockIdFor(LockParams calldata p, address payer, address tokenIn, uint256 amountIn)
        public
        view
        returns (bytes32)
    {
        return keccak256(abi.encode(LOCK_TAG, block.chainid, address(this), p, payer, tokenIn, amountIn));
    }

    /// @notice Locks the payer's USDC using their signed authorization. Anyone may submit it.
    function lockWithAuthorization(
        LockParams calldata p,
        address payer,
        address tokenIn,
        uint256 amountIn,
        uint256 validAfter,
        uint256 validBefore,
        uint8 v,
        bytes32 r,
        bytes32 s,
        bytes calldata swapData
    ) external returns (bytes32 lockId) {
        if (tokenIn != address(usdc)) revert UnsupportedToken(tokenIn);
        if (swapData.length != 0) revert SwapNotSupported();
        if (validBefore > block.timestamp + MAX_AUTH_WINDOW) {
            revert AuthorizationWindowTooLong(validBefore, block.timestamp + MAX_AUTH_WINDOW);
        }
        lockId = lockIdFor(p, payer, tokenIn, amountIn);
        _open(lockId, p, payer, amountIn);
        IERC3009(address(usdc))
            .receiveWithAuthorization(payer, address(this), amountIn, validAfter, validBefore, lockId, v, r, s);
    }

    /// @notice Locks the caller's USDC (approved beforehand). Used by the provider for refunds.
    function lockFromProvider(LockParams calldata p, uint256 usdcAmount) external returns (bytes32 lockId) {
        lockId = lockIdFor(p, msg.sender, address(usdc), usdcAmount);
        _open(lockId, p, msg.sender, usdcAmount);
        usdc.safeTransferFrom(msg.sender, address(this), usdcAmount);
    }

    /// @notice Source redeem: pays the receiver if `secret` matches the hashlock before expiry.
    function claim(bytes32 lockId, bytes32 secret) external {
        Lock storage l = locks[lockId];
        if (l.state != State.Open) revert NotOpen(lockId);
        if (block.timestamp >= l.expiry) revert LockExpired(lockId);
        if (keccak256(abi.encodePacked(secret)) != l.hashlock) revert WrongSecret(lockId);
        l.state = State.Claimed;
        usdc.safeTransfer(l.receiver, l.amount);
        emit Claimed(lockId, secret);
    }

    /// @notice Returns the USDC to the payer once the lock has expired. Anyone may call it.
    function refund(bytes32 lockId) external {
        Lock storage l = locks[lockId];
        if (l.state != State.Open) revert NotOpen(lockId);
        if (block.timestamp < l.expiry) revert NotExpired(lockId);
        l.state = State.Refunded;
        usdc.safeTransfer(l.payer, l.amount);
        emit Refunded(lockId);
    }

    function _open(bytes32 lockId, LockParams calldata p, address payer, uint256 amount) private {
        if (amount == 0) revert ZeroAmount();
        if (amount > type(uint96).max) revert AmountTooLarge(amount);
        if (amount < p.minUsdcOut) revert BelowMinimum(amount, p.minUsdcOut);
        if (p.receiver == address(0)) revert ZeroReceiver();
        if (p.hashlock == bytes32(0)) revert ZeroHashlock();
        if (p.expiry < block.timestamp + MIN_LOCK_TIME || p.expiry > block.timestamp + MAX_LOCK_TIME) {
            revert ExpiryOutOfRange(p.expiry);
        }
        if (p.outToken != address(0) && p.outToken != address(usdc)) revert UnsupportedToken(p.outToken);
        if (locks[lockId].state != State.None) revert LockExists(lockId);

        locks[lockId] = Lock({
            payer: payer,
            expiry: p.expiry,
            state: State.Open,
            receiver: p.receiver,
            amount: uint96(amount),
            hashlock: p.hashlock,
            orderId: p.orderId
        });
        emit Locked(lockId, p.orderId, payer, p.receiver, amount, p.hashlock, p.expiry);
    }
}
```

**Step 2: Run the unit tests**

Run: `cd contracts && forge test --match-contract SourceLockTest`
Expected: PASS, 15 tests (including the 256-run fuzz test).

**Step 3: Run the whole suite to check nothing else broke**

Run: `cd contracts && forge test`
Expected: PASS, every existing DirectPay and Aqua test plus the new ones.

**Step 4: Commit**

```bash
git add contracts/src/SourceLock.sol contracts/test/SourceLock.t.sol
git commit -m "Add SourceLock: gasless USDC source lock with hashlock, expiry and refund"
```

---

### Task 3: Test against the real Base Sepolia USDC (fork, nothing sent)

**Files:**
- Test: `contracts/test/SourceLock.fork.t.sol`

**Step 1: Write the fork test**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SourceLock} from "../src/SourceLock.sol";
import {Sign3009} from "./utils/Sign3009.sol";

/// Runs SourceLock against the real Base Sepolia USDC on a local fork. Nothing is sent.
/// Run with: forge test --match-contract SourceLockForkTest
contract SourceLockForkTest is Sign3009 {
    IERC20 constant USDC = IERC20(0x036CbD53842c5426634e7929541eC2318f3dCF7e);

    SourceLock lock;
    uint256 customerKey = 0xA11CE;
    address customer;
    address bob = makeAddr("bob");

    function setUp() public {
        vm.createSelectFork("base_sepolia");
        customer = vm.addr(customerKey);
        lock = new SourceLock(USDC);
        deal(address(USDC), customer, 50e6);
    }

    function test_domainSeparatorMatchesWalletDomain() public view {
        bytes32 expected = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("USDC"),
                keccak256("2"),
                uint256(84532),
                address(USDC)
            )
        );
        (bool ok, bytes memory data) = address(USDC).staticcall(abi.encodeWithSignature("DOMAIN_SEPARATOR()"));
        assertTrue(ok);
        assertEq(abi.decode(data, (bytes32)), expected);
    }

    function test_lockAndClaimWithRealUsdc() public {
        bytes32 secret = keccak256("fork-secret");
        SourceLock.LockParams memory p = SourceLock.LockParams({
            orderId: keccak256("order-3001"),
            receiver: bob,
            minUsdcOut: 12e6,
            hashlock: keccak256(abi.encodePacked(secret)),
            expiry: uint64(block.timestamp + 20 minutes),
            outToken: address(0)
        });
        bytes32 nonce = lock.lockIdFor(p, customer, address(USDC), 12e6);
        Auth memory a = signReceive(
            address(USDC), customerKey, address(lock), 12e6, block.timestamp - 1, block.timestamp + 600, nonce
        );
        bytes32 id = lock.lockWithAuthorization(
            p, customer, address(USDC), 12e6, a.validAfter, a.validBefore, a.v, a.r, a.s, ""
        );
        assertEq(USDC.balanceOf(address(lock)), 12e6);

        lock.claim(id, secret);
        assertEq(USDC.balanceOf(bob), 12e6);
    }
}
```

**Step 2: Run it**

Run: `cd contracts && forge test --match-contract SourceLockForkTest`
Expected: PASS, 2 tests. The first confirms the wallet signing domain (`USDC`, version `2`, chain 84532). The second locks and redeems 12 USDC against the real token.

**Step 3: Commit**

```bash
git add contracts/test/SourceLock.fork.t.sol
git commit -m "Test SourceLock against real Base Sepolia USDC on a fork"
```

---

### Task 4: Deploy and try scripts

**Files:**
- Create: `contracts/script/DeploySourceLock.s.sol`
- Create: `contracts/script/TrySourceLock.s.sol`

**Step 1: Write the deploy script**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Script, console} from "forge-std/Script.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SourceLock} from "../src/SourceLock.sol";

/// Deploys SourceLock for the chain's Circle USDC (Base Sepolia or Ethereum Sepolia).
contract DeploySourceLock is Script {
    function run() external returns (SourceLock sourceLock) {
        address usdc;
        if (block.chainid == 84532) usdc = 0x036CbD53842c5426634e7929541eC2318f3dCF7e;
        else if (block.chainid == 11155111) usdc = 0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238;
        else revert("unsupported chain");
        vm.startBroadcast();
        sourceLock = new SourceLock(IERC20(usdc));
        vm.stopBroadcast();
        console.log("SourceLock:", address(sourceLock));
    }
}
```

**Step 2: Write the try script**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Script, console} from "forge-std/Script.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SourceLock} from "../src/SourceLock.sol";

interface IDomain {
    function DOMAIN_SEPARATOR() external view returns (bytes32);
}

/// End-to-end check of SourceLock on Base Sepolia with test wallets: the customer signs a lock
/// for the receiver, the gas payer submits it, then claims it with the secret (source redeem).
/// Reads CUSTOMER_PRIVATE_KEY, GAS_PAYER_PRIVATE_KEY, SOURCE_LOCK and optional AMOUNT and
/// RECEIVER (defaults to the shop test wallet) from the env.
contract TrySourceLock is Script {
    IERC20 constant USDC = IERC20(0x036CbD53842c5426634e7929541eC2318f3dCF7e);
    bytes32 constant RECEIVE_TYPEHASH = keccak256(
        "ReceiveWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)"
    );

    function run() external {
        require(block.chainid == 84532, "not Base Sepolia");
        SourceLock sourceLock = SourceLock(vm.envAddress("SOURCE_LOCK"));
        uint256 customerKey = vm.envUint("CUSTOMER_PRIVATE_KEY");
        uint256 gasPayerKey = vm.envUint("GAS_PAYER_PRIVATE_KEY");
        address customer = vm.addr(customerKey);
        address receiver = vm.envOr("RECEIVER", vm.addr(vm.envUint("SHOP_PRIVATE_KEY")));
        uint256 amount = vm.envOr("AMOUNT", uint256(1e6));

        bytes32 secret = keccak256(abi.encode("uniform-test-secret", block.timestamp));
        SourceLock.LockParams memory p = SourceLock.LockParams({
            orderId: keccak256(abi.encode("uniform-test-order", block.timestamp)),
            receiver: receiver,
            minUsdcOut: amount,
            hashlock: keccak256(abi.encodePacked(secret)),
            expiry: uint64(block.timestamp + 20 minutes),
            outToken: address(0)
        });
        uint256 validAfter = block.timestamp - 60;
        uint256 validBefore = block.timestamp + 600;
        bytes32 nonce = sourceLock.lockIdFor(p, customer, address(USDC), amount);
        bytes32 digest = keccak256(
            abi.encodePacked(
                "\x19\x01",
                IDomain(address(USDC)).DOMAIN_SEPARATOR(),
                keccak256(
                    abi.encode(
                        RECEIVE_TYPEHASH, customer, address(sourceLock), amount, validAfter, validBefore, nonce
                    )
                )
            )
        );
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(customerKey, digest);

        console.log("receiver USDC before:", USDC.balanceOf(receiver));
        vm.startBroadcast(gasPayerKey);
        bytes32 lockId = sourceLock.lockWithAuthorization(
            p, customer, address(USDC), amount, validAfter, validBefore, v, r, s, ""
        );
        sourceLock.claim(lockId, secret);
        vm.stopBroadcast();
        console.log("lockId:");
        console.logBytes32(lockId);
        console.log("receiver USDC after:", USDC.balanceOf(receiver));
    }
}
```

**Step 3: Build**

Run: `cd contracts && forge build`
Expected: `Compiler run successful`.

**Step 4: Simulate the deploy (free, nothing sent)**

Run: `cd contracts && forge script script/DeploySourceLock.s.sol --rpc-url base_sepolia`
Expected: `Script ran successfully` and a `SourceLock:` address. Without `--broadcast` nothing is sent. The full lock-and-redeem flow is already covered by the Task 3 fork test.

**Step 5: Commit**

```bash
git add contracts/script/DeploySourceLock.s.sol contracts/script/TrySourceLock.s.sol
git commit -m "Add SourceLock deploy and end-to-end try scripts"
```

---

### Task 5: Deploy and verify on Base Sepolia (real, costs a little Base ETH)

**Step 1: The user runs this** (it needs the keystore password)

```bash
cd contracts && forge script script/DeploySourceLock.s.sol --rpc-url base_sepolia --account deployer --broadcast --verify --verifier blockscout --verifier-url https://base-sepolia.blockscout.com/api/
```

Expected: `SourceLock: 0x…`, then "Contract successfully verified".

**Step 2: Record the address**

Put it in `shared/chains.json` → `baseSepolia.contracts.sourceLock`.

---

### Task 6: Live test on Base Sepolia

**Step 1: The user funds the test wallets from the deployer**

- 1 USDC to the test customer `0xC2bF8030f00CAB26c4611f93c5F71b9d01Ec120A`
- 0.005 Base Sepolia ETH to the gas payer `0x18F0339Ab5e661889b9f8653AE20A686Ff800890`

**Step 2: Run the try script**

```bash
cd contracts && set -a && . ../.env.testwallets && set +a && SOURCE_LOCK=<address from Task 5> forge script script/TrySourceLock.s.sol --rpc-url base_sepolia --broadcast
```

Expected: `ONCHAIN EXECUTION COMPLETE & SUCCESSFUL`, and the receiver's USDC goes up by 1. On Basescan, the lock transaction shows a `Locked` event and the claim transaction shows `Claimed` with the secret.

**Step 3: Commit the broadcast record**

```bash
git add contracts/broadcast/DeploySourceLock.s.sol contracts/broadcast/TrySourceLock.s.sol shared/chains.json
git commit -m "Deploy SourceLock on Base Sepolia and record the first live lock and redeem"
```

---

### Task 7: Update the spec and hand over to B

**Step 1:** In `docs/SPEC.md` → SourceLock, replace the nonce line with the lock-id formula from this plan, and add `lockIdFor(LockParams p, address payer, address tokenIn, uint256 amountIn) view returns (bytes32)` plus the limits (5 min to 3 h, 1 h authorization window).

**Step 2:** Export the ABI: `jq .abi contracts/out/SourceLock.sol/SourceLock.json > shared/abis/SourceLock.json`.

**Step 3:** Mark A3 ✅ in the build plan's progress table with the two transaction links. Tell B that B4 can build the signing data:

```
domain  = { name: "USDC", version: "2", chainId: 84532, verifyingContract: USDC }
types   = ReceiveWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)
message = { from: customer, to: SourceLock, value: amountIn, validAfter: now - 60,
            validBefore: now + 600, nonce: lockIdFor(params, customer, USDC, amountIn) }
```

**Step 4: Commit and push**

```bash
git add docs/SPEC.md shared/abis/SourceLock.json
git commit -m "Spec: SourceLock lock id, limits and ABI"
git push origin dev
```
