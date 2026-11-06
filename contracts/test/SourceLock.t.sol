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
