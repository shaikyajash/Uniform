// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SourceLock} from "../src/SourceLock.sol";
import {MockEIP3009} from "./mocks/MockEIP3009.sol";
import {Sign3009} from "./utils/Sign3009.sol";

contract SourceLockTest is Sign3009 {
    MockEIP3009 usdc;
    MockEIP3009 eurc;
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
        eurc = new MockEIP3009();
        lock = new SourceLock(IERC20(address(usdc)), IERC20(address(eurc)));
        usdc.mint(customer, 50e6);
        eurc.mint(customer, 50e6);
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
            address(usdc), customerKey, address(lock), amount, block.timestamp - 1, block.timestamp + 5 minutes, nonce
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
        usdc.mint(bob, 10e6);
        vm.prank(bob);
        usdc.approve(address(lock), 10e6);

        SourceLock.LockParams memory p = _params(1e6);
        p.expiry = uint64(block.timestamp + 1 minutes);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.ExpiryOutOfRange.selector, p.expiry));
        vm.prank(bob);
        lock.lock(p, address(usdc), 1e6);

        p.expiry = uint64(block.timestamp + 4 hours);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.ExpiryOutOfRange.selector, p.expiry));
        vm.prank(bob);
        lock.lock(p, address(usdc), 1e6);
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
        address dai = makeAddr("dai");
        vm.expectRevert(abi.encodeWithSelector(SourceLock.UnsupportedToken.selector, dai));
        lock.lockWithAuthorization(p, customer, dai, 12e6, a.validAfter, a.validBefore, a.v, a.r, a.s, "");

        vm.expectRevert(SourceLock.SwapNotSupported.selector);
        lock.lockWithAuthorization(
            p, customer, address(usdc), 12e6, a.validAfter, a.validBefore, a.v, a.r, a.s, hex"01"
        );
    }

    function test_rejectsLongAuthorizationWindow() public {
        SourceLock.LockParams memory p = _params(12e6);
        p.expiry = uint64(block.timestamp + 3 hours);
        bytes32 nonce = lock.lockIdFor(p, customer, address(usdc), 12e6);
        Auth memory a = signReceive(
            address(usdc), customerKey, address(lock), 12e6, block.timestamp - 1, block.timestamp + 2 hours, nonce
        );
        vm.expectRevert(
            abi.encodeWithSelector(SourceLock.AuthorizationWindowTooLong.selector, a.validAfter, a.validBefore)
        );
        _lock(p, 12e6, a);
    }

    function test_rejectsAuthorizationEndingTooCloseToExpiry() public {
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 nonce = lock.lockIdFor(p, customer, address(usdc), 12e6);
        Auth memory a = signReceive(
            address(usdc), customerKey, address(lock), 12e6, block.timestamp - 1, block.timestamp + 6 minutes, nonce
        );
        vm.expectRevert(abi.encodeWithSelector(SourceLock.AuthorizationTooLate.selector, a.validBefore, p.expiry));
        _lock(p, 12e6, a);
    }

    function test_lateSubmissionStillLeavesTimeForTheDestination() public {
        SourceLock.LockParams memory p = _params(12e6);
        Auth memory a = _sign(p, 12e6);
        vm.warp(a.validBefore - 1);
        bytes32 id = _lock(p, 12e6, a);
        (,,,,,, bytes32 orderId,) = lock.locks(id);
        assertEq(orderId, p.orderId);
        assertGe(p.expiry - block.timestamp, lock.MIN_TIME_AFTER_AUTH());
    }

    function test_rejectsReceiverThatWouldStrandFunds() public {
        SourceLock.LockParams memory p = _params(12e6);
        p.receiver = address(lock);
        Auth memory a = _sign(p, 12e6);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.InvalidReceiver.selector, address(lock)));
        _lock(p, 12e6, a);

        p.receiver = address(usdc);
        a = _sign(p, 12e6);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.InvalidReceiver.selector, address(usdc)));
        _lock(p, 12e6, a);
    }

    function test_rejectsZeroReceiverHashlockAndAmount() public {
        SourceLock.LockParams memory p = _params(0);
        p.receiver = address(0);
        Auth memory a = _sign(p, 12e6);
        vm.expectRevert(SourceLock.ZeroReceiver.selector);
        _lock(p, 12e6, a);

        p = _params(0);
        p.hashlock = bytes32(0);
        a = _sign(p, 12e6);
        vm.expectRevert(SourceLock.ZeroHashlock.selector);
        _lock(p, 12e6, a);

        p = _params(0);
        a = _sign(p, 0);
        vm.expectRevert(SourceLock.ZeroAmount.selector);
        _lock(p, 0, a);
    }

    function test_acceptsUsdcAsOutToken() public {
        SourceLock.LockParams memory p = _params(12e6);
        p.outToken = address(usdc);
        bytes32 id = _lock(p, 12e6, _sign(p, 12e6));
        lock.claim(id, secret);
        assertEq(usdc.balanceOf(bob), 12e6);
    }

    function test_submitterCannotChangeAmount() public {
        SourceLock.LockParams memory p = _params(5e6);
        Auth memory a = _sign(p, 12e6);
        vm.expectRevert("invalid signature");
        _lock(p, 6e6, a);
    }

    function test_signatureForAnotherLockContractFails() public {
        SourceLock other = new SourceLock(IERC20(address(usdc)), IERC20(address(eurc)));
        SourceLock.LockParams memory p = _params(12e6);
        Auth memory a = _sign(p, 12e6);
        vm.expectRevert("invalid signature");
        other.lockWithAuthorization(p, customer, address(usdc), 12e6, a.validAfter, a.validBefore, a.v, a.r, a.s, "");
    }

    function test_eventsCarryTheLockDetails() public {
        SourceLock.LockParams memory p = _params(12e6);
        Auth memory a = _sign(p, 12e6);
        bytes32 id = lock.lockIdFor(p, customer, address(usdc), 12e6);
        vm.expectEmit(address(lock));
        emit SourceLock.Locked(id, p.orderId, customer, bob, address(usdc), 12e6, hashlock, p.expiry);
        _lock(p, 12e6, a);

        vm.expectEmit(address(lock));
        emit SourceLock.Claimed(id, secret);
        lock.claim(id, secret);
    }

    function test_anyoneCanRefundAfterExpiry() public {
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 id = _lock(p, 12e6, _sign(p, 12e6));
        vm.warp(p.expiry);
        vm.expectEmit(address(lock));
        emit SourceLock.Refunded(id);
        vm.prank(makeAddr("stranger"));
        lock.refund(id);
        assertEq(usdc.balanceOf(customer), 50e6);
    }

    function test_providerLockRefundsToProvider() public {
        usdc.mint(bob, 4e6);
        SourceLock.LockParams memory p = _params(4e6);
        p.receiver = customer;
        vm.startPrank(bob);
        usdc.approve(address(lock), 4e6);
        bytes32 id = lock.lock(p, address(usdc), 4e6);
        vm.stopPrank();
        vm.warp(p.expiry);
        lock.refund(id);
        assertEq(usdc.balanceOf(bob), 4e6);
    }

    function test_payerLocksItsOwnTokensDirectly() public {
        usdc.mint(bob, 5e6);
        SourceLock.LockParams memory p = _params(4e6);
        p.receiver = customer;
        vm.startPrank(bob);
        usdc.approve(address(lock), 4e6);
        bytes32 id = lock.lock(p, address(usdc), 4e6);
        vm.stopPrank();

        lock.claim(id, secret);
        assertEq(usdc.balanceOf(customer), 54e6);
        assertEq(usdc.balanceOf(bob), 1e6);
    }

    function _signEurc(SourceLock.LockParams memory p, uint256 amount) internal view returns (Auth memory) {
        bytes32 nonce = lock.lockIdFor(p, customer, address(eurc), amount);
        return signReceive(
            address(eurc), customerKey, address(lock), amount, block.timestamp - 1, block.timestamp + 5 minutes, nonce
        );
    }

    function _lockEurc(SourceLock.LockParams memory p, uint256 amount, Auth memory a) internal returns (bytes32) {
        vm.prank(gasPayer);
        return lock.lockWithAuthorization(p, customer, address(eurc), amount, a.validAfter, a.validBefore, a.v, a.r, a.s, "");
    }

    function test_eurcIsLockedAsEurcAndClaimedAsEurc() public {
        // 10.85 EURC for a 12 USDC order: the USDC value is checked off chain, not by the lock.
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 id = _lockEurc(p, 10_850_000, _signEurc(p, 10_850_000));
        (,,,,,,, address token) = lock.locks(id);
        assertEq(token, address(eurc));
        assertEq(eurc.balanceOf(address(lock)), 10_850_000);
        assertEq(usdc.balanceOf(address(lock)), 0);

        lock.claim(id, secret);
        assertEq(eurc.balanceOf(bob), 10_850_000);
        assertEq(usdc.balanceOf(bob), 0);
    }

    function test_anExpiredEurcLockRefundsTheSameEurc() public {
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 id = _lockEurc(p, 10_850_000, _signEurc(p, 10_850_000));
        vm.warp(p.expiry);
        lock.refund(id);
        assertEq(eurc.balanceOf(customer), 50e6);
        assertEq(eurc.balanceOf(address(lock)), 0);
    }

    function test_usdcAndEurcLocksStayApart() public {
        SourceLock.LockParams memory p = _params(5e6);
        bytes32 usdcId = _lock(p, 5e6, _sign(p, 5e6));
        bytes32 eurcId = _lockEurc(p, 4e6, _signEurc(p, 4e6));
        assertTrue(usdcId != eurcId, "token is part of the lock id");
        vm.warp(p.expiry);
        lock.refund(eurcId);
        assertEq(usdc.balanceOf(address(lock)), 5e6);
        assertEq(eurc.balanceOf(customer), 50e6);
    }

    function test_aUsdcSignatureCannotLockEurc() public {
        SourceLock.LockParams memory p = _params(5e6);
        Auth memory a = _sign(p, 5e6);
        vm.expectRevert("invalid signature");
        _lockEurc(p, 5e6, a);
    }

    function test_directLockOfEurcAndUnsupportedTokens() public {
        SourceLock.LockParams memory p = _params(0);
        vm.startPrank(customer);
        eurc.approve(address(lock), 3e6);
        bytes32 id = lock.lock(p, address(eurc), 3e6);
        MockEIP3009 other = new MockEIP3009();
        vm.expectRevert(abi.encodeWithSelector(SourceLock.UnsupportedToken.selector, address(other)));
        lock.lock(p, address(other), 1e6);
        vm.stopPrank();
        lock.claim(id, secret);
        assertEq(eurc.balanceOf(bob), 3e6);
    }

    function test_rejectsEitherAllowedTokenAsReceiver() public {
        SourceLock.LockParams memory p = _params(1e6);
        p.receiver = address(eurc);
        Auth memory a = _sign(p, 1e6);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.InvalidReceiver.selector, address(eurc)));
        _lock(p, 1e6, a);
    }

    function test_constructorRejectsBadTokens() public {
        vm.expectRevert(SourceLock.ZeroToken.selector);
        new SourceLock(IERC20(address(usdc)), IERC20(address(0)));
        vm.expectRevert(SourceLock.ZeroToken.selector);
        new SourceLock(IERC20(address(usdc)), IERC20(address(usdc)));
    }

    function test_amountAboveUint96IsRejected() public {
        uint256 huge = uint256(type(uint96).max) + 1;
        usdc.mint(customer, huge);
        SourceLock.LockParams memory p = _params(0);
        vm.startPrank(customer);
        usdc.approve(address(lock), huge);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.AmountTooLarge.selector, huge));
        lock.lock(p, address(usdc), huge);
        vm.stopPrank();
    }

    function test_rejectsAnOutTokenOtherThanUsdc() public {
        SourceLock.LockParams memory p = _params(1e6);
        p.outToken = address(eurc);
        Auth memory a = _sign(p, 1e6);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.UnsupportedToken.selector, address(eurc)));
        _lock(p, 1e6, a);
    }

    function test_exactLimitsAreAccepted() public {
        // Lock time of exactly 3 hours through lock(); exactly 5 minutes too.
        SourceLock.LockParams memory p = _params(0);
        vm.startPrank(customer);
        usdc.approve(address(lock), 2e6);
        p.expiry = uint64(block.timestamp + 3 hours);
        lock.lock(p, address(usdc), 1e6);
        p.expiry = uint64(block.timestamp + 5 minutes);
        lock.lock(p, address(usdc), 1e6);
        vm.stopPrank();

        // An authorization of exactly 1 hour that ends exactly 15 minutes before expiry.
        SourceLock.LockParams memory q = _params(1e6);
        q.expiry = uint64(block.timestamp + 1 hours + 15 minutes);
        bytes32 nonce = lock.lockIdFor(q, customer, address(usdc), 1e6);
        Auth memory a =
            signReceive(address(usdc), customerKey, address(lock), 1e6, block.timestamp - 1, block.timestamp - 1 + 1 hours, nonce);
        q.expiry = uint64(a.validBefore + 15 minutes);
        nonce = lock.lockIdFor(q, customer, address(usdc), 1e6);
        a = signReceive(address(usdc), customerKey, address(lock), 1e6, block.timestamp - 1, block.timestamp - 1 + 1 hours, nonce);
        _lock(q, 1e6, a);
    }

    function test_aSignatureOnlyWorksForItsSigner() public {
        // The Gas Payer names someone else as payer: the nonce and the signer no longer match.
        SourceLock.LockParams memory p = _params(5e6);
        Auth memory a = _sign(p, 5e6);
        address other = makeAddr("other");
        usdc.mint(other, 5e6);
        vm.prank(gasPayer);
        vm.expectRevert("invalid signature");
        lock.lockWithAuthorization(p, other, address(usdc), 5e6, a.validAfter, a.validBefore, a.v, a.r, a.s, "");
    }

    function test_directLockChecksReuseAndTheUsdcMinimum() public {
        SourceLock.LockParams memory p = _params(2e6);
        vm.startPrank(customer);
        usdc.approve(address(lock), 10e6);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.BelowMinimum.selector, 1e6, 2e6));
        lock.lock(p, address(usdc), 1e6);
        bytes32 id = lock.lock(p, address(usdc), 2e6);
        vm.expectRevert(abi.encodeWithSelector(SourceLock.LockExists.selector, id));
        lock.lock(p, address(usdc), 2e6);
        vm.stopPrank();
    }

    function test_aThirdPartyCanClaimButOnlyTheReceiverIsPaid() public {
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 id = _lock(p, 12e6, _sign(p, 12e6));
        address stranger = makeAddr("stranger");
        vm.prank(stranger);
        lock.claim(id, secret);
        assertEq(usdc.balanceOf(bob), 12e6);
        assertEq(usdc.balanceOf(stranger), 0);
    }

    function test_payerRacingTheRelayerOpensTheSameLockOnce() public {
        SourceLock.LockParams memory p = _params(5e6);
        Auth memory a = _sign(p, 5e6);
        vm.startPrank(customer);
        usdc.approve(address(lock), 5e6);
        bytes32 id = lock.lock(p, address(usdc), 5e6);
        vm.stopPrank();
        vm.expectRevert(abi.encodeWithSelector(SourceLock.LockExists.selector, id));
        _lock(p, 5e6, a);
        vm.warp(p.expiry);
        lock.refund(id);
        vm.expectRevert();
        _lock(p, 5e6, a);
        (,, SourceLock.State state,,,,,) = lock.locks(id);
        assertEq(uint8(state), uint8(SourceLock.State.Refunded));
    }

    /// Any mix of USDC and EURC locks, claimed or refunded, pays each party exactly its own token
    /// and leaves nothing behind.
    function testFuzz_eachTokenStaysExact(uint8 count, uint256 seed) public {
        count = uint8(bound(count, 1, 12));
        bytes32[] memory ids = new bytes32[](count);
        bool[] memory isEurc = new bool[](count);
        uint256[] memory amounts = new uint256[](count);
        bool[] memory claimIt = new bool[](count);
        uint256 bobUsdc;
        uint256 bobEurc;
        for (uint256 k; k < count; k++) {
            uint256 r = uint256(keccak256(abi.encode(seed, k)));
            isEurc[k] = r % 2 == 0;
            amounts[k] = 1 + (r >> 8) % 3e6;
            claimIt[k] = (r >> 64) % 2 == 0;
            SourceLock.LockParams memory p = _params(0);
            p.orderId = bytes32(k);
            ids[k] = isEurc[k]
                ? _lockEurc(p, amounts[k], _signEurc(p, amounts[k]))
                : _lock(p, amounts[k], _sign(p, amounts[k]));
            if (claimIt[k]) {
                if (isEurc[k]) bobEurc += amounts[k];
                else bobUsdc += amounts[k];
            }
        }
        for (uint256 k; k < count; k++) {
            if (claimIt[k]) lock.claim(ids[k], secret);
        }
        vm.warp(block.timestamp + 20 minutes);
        for (uint256 k; k < count; k++) {
            if (!claimIt[k]) lock.refund(ids[k]);
        }
        assertEq(usdc.balanceOf(address(lock)), 0);
        assertEq(eurc.balanceOf(address(lock)), 0);
        assertEq(usdc.balanceOf(bob), bobUsdc);
        assertEq(eurc.balanceOf(bob), bobEurc);
        assertEq(usdc.balanceOf(customer), 50e6 - bobUsdc);
        assertEq(eurc.balanceOf(customer), 50e6 - bobEurc);
    }

    function testFuzz_onlyTheSecretOpensTheLock(bytes32 guess) public {
        vm.assume(guess != secret);
        SourceLock.LockParams memory p = _params(12e6);
        bytes32 id = _lock(p, 12e6, _sign(p, 12e6));
        vm.expectRevert(abi.encodeWithSelector(SourceLock.WrongSecret.selector, id));
        lock.claim(id, guess);
    }
}
