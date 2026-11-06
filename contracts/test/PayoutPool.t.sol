// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Test} from "forge-std/Test.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {AquaRouter} from "@1inch/aqua/contracts/AquaRouter.sol";
import {IAqua} from "@1inch/aqua/contracts/interfaces/IAqua.sol";
import {PayoutPool} from "../src/PayoutPool.sol";
import {DestinationLock} from "../src/DestinationLock.sol";
import {MockEIP3009} from "./mocks/MockEIP3009.sol";

contract PayoutPoolTest is Test {
    AquaRouter aqua;
    MockEIP3009 usdt;
    PayoutPool pool;
    DestinationLock lock;

    address bob = makeAddr("bob");
    address filler = makeAddr("matchmaker");
    address shop = makeAddr("shop");
    bytes32 secret = keccak256("mei-secret");
    bytes32 hashlock = keccak256(abi.encodePacked(keccak256("mei-secret")));
    uint256 constant SRC_CHAIN = 84532;

    PayoutPool.Strategy strategy;
    bytes32 strategyHash;

    function setUp() public {
        vm.warp(1_800_000_000);
        aqua = new AquaRouter(address(this));
        usdt = new MockEIP3009();
        pool = new PayoutPool(IAqua(address(aqua)), IERC20(address(usdt)));
        lock = pool.destinationLock();

        strategy = PayoutPool.Strategy({
            maker: bob, token: address(usdt), filler: filler, maxPerOrder: 20e6, feeBps: 30, salt: bytes32("bob-1")
        });
        strategyHash = _ship(strategy, 50e6);
    }

    function _ship(PayoutPool.Strategy memory s, uint256 allowance) internal returns (bytes32) {
        usdt.mint(s.maker, 100e6);
        vm.startPrank(s.maker);
        usdt.approve(address(aqua), type(uint256).max);
        address[] memory tokens = new address[](1);
        tokens[0] = address(usdt);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = allowance;
        bytes32 h = aqua.ship(address(pool), abi.encode(s), tokens, amounts);
        vm.stopPrank();
        return h;
    }

    function _fill(bytes32 srcLockId, uint256 amount) internal returns (bytes32) {
        PayoutPool.Fill memory f = PayoutPool.Fill({
            orderId: keccak256("order-1002"),
            merchant: shop,
            amount: amount,
            hashlock: hashlock,
            expiry: uint64(block.timestamp + 10 minutes),
            srcChainId: SRC_CHAIN,
            srcLockId: srcLockId
        });
        vm.prank(filler);
        return pool.fill(strategy, f);
    }

    function _allowance() internal view returns (uint256 a) {
        (a,) = aqua.rawBalances(bob, address(pool), strategyHash, address(usdt));
    }

    function test_strategyHashMatchesAquaShip() public view {
        assertEq(pool.strategyHashOf(strategy), strategyHash);
    }

    function test_fillMovesBobsUsdtIntoTheLock() public {
        bytes32 id = _fill(keccak256("src-1"), 12e6);
        assertEq(usdt.balanceOf(address(lock)), 12e6);
        assertEq(usdt.balanceOf(bob), 88e6);
        assertEq(_allowance(), 38e6);
        assertEq(id, lock.lockIdFor(SRC_CHAIN, keccak256("src-1")));
    }

    function test_onlyTheFillerCanFill() public {
        PayoutPool.Fill memory f = PayoutPool.Fill({
            orderId: keccak256("order-1002"),
            merchant: shop,
            amount: 12e6,
            hashlock: hashlock,
            expiry: uint64(block.timestamp + 10 minutes),
            srcChainId: SRC_CHAIN,
            srcLockId: keccak256("src-1")
        });
        vm.expectRevert(abi.encodeWithSelector(PayoutPool.NotFiller.selector, address(this)));
        pool.fill(strategy, f);
    }

    function test_fillAboveMaxPerOrderReverts() public {
        vm.expectRevert(abi.encodeWithSelector(PayoutPool.AboveMaxPerOrder.selector, 21e6, 20e6));
        _fill(keccak256("src-1"), 21e6);
    }

    function test_fillWithOtherTokenReverts() public {
        strategy.token = makeAddr("eurc");
        vm.expectRevert(abi.encodeWithSelector(PayoutPool.WrongToken.selector, strategy.token));
        _fill(keccak256("src-1"), 12e6);
    }

    function test_unshippedStrategyCannotFill() public {
        strategy.salt = bytes32("never-shipped");
        vm.expectRevert();
        _fill(keccak256("src-1"), 12e6);
    }

    function test_dockedStrategyCannotFill() public {
        address[] memory tokens = new address[](1);
        tokens[0] = address(usdt);
        vm.prank(bob);
        aqua.dock(address(pool), strategyHash, tokens);
        vm.expectRevert();
        _fill(keccak256("src-1"), 12e6);
    }

    function test_sameSourceLockCannotBeFilledTwice() public {
        bytes32 id = _fill(keccak256("src-1"), 12e6);
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.LockExists.selector, id));
        _fill(keccak256("src-1"), 12e6);
    }

    function test_onlyThePoolCanOpen() public {
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.NotPool.selector, address(this)));
        lock.open(
            DestinationLock.OpenParams({
                orderId: keccak256("order-1002"),
                merchant: shop,
                amount: 12e6,
                hashlock: hashlock,
                expiry: uint64(block.timestamp + 10 minutes),
                srcChainId: SRC_CHAIN,
                srcLockId: keccak256("src-1"),
                maker: bob,
                strategyHash: strategyHash
            })
        );
    }

    function test_openRejectsBadMerchantHashlockExpiryAndAmount() public {
        PayoutPool.Fill memory f = PayoutPool.Fill({
            orderId: keccak256("order-1002"),
            merchant: address(0),
            amount: 12e6,
            hashlock: hashlock,
            expiry: uint64(block.timestamp + 10 minutes),
            srcChainId: SRC_CHAIN,
            srcLockId: keccak256("src-1")
        });
        vm.startPrank(filler);
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.InvalidMerchant.selector, address(0)));
        pool.fill(strategy, f);

        f.merchant = address(lock);
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.InvalidMerchant.selector, address(lock)));
        pool.fill(strategy, f);

        f.merchant = address(usdt);
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.InvalidMerchant.selector, address(usdt)));
        pool.fill(strategy, f);

        f.merchant = shop;
        f.hashlock = bytes32(0);
        vm.expectRevert(DestinationLock.ZeroHashlock.selector);
        pool.fill(strategy, f);

        f.hashlock = hashlock;
        f.expiry = uint64(block.timestamp + 1 minutes);
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.ExpiryOutOfRange.selector, f.expiry));
        pool.fill(strategy, f);

        f.expiry = uint64(block.timestamp + 3 hours);
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.ExpiryOutOfRange.selector, f.expiry));
        pool.fill(strategy, f);

        f.expiry = uint64(block.timestamp + 10 minutes);
        f.amount = 0;
        vm.expectRevert(DestinationLock.ZeroAmount.selector);
        pool.fill(strategy, f);
        vm.stopPrank();
    }

    function test_claimPaysTheShopAndRevealsTheSecret() public {
        bytes32 id = _fill(keccak256("src-1"), 12e6);
        vm.expectEmit(address(lock));
        emit DestinationLock.Settled(id, keccak256("order-1002"), shop, 12e6, SRC_CHAIN, keccak256("src-1"), secret);
        vm.prank(makeAddr("gasPayer"));
        lock.claim(id, secret);
        assertEq(usdt.balanceOf(shop), 12e6);
        assertEq(usdt.balanceOf(address(lock)), 0);
    }

    function test_claimRejectsWrongSecret() public {
        bytes32 id = _fill(keccak256("src-1"), 12e6);
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.WrongSecret.selector, id));
        lock.claim(id, keccak256("guess"));
    }

    function test_claimAtExpiryReverts() public {
        bytes32 id = _fill(keccak256("src-1"), 12e6);
        vm.warp(block.timestamp + 10 minutes);
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.LockExpired.selector, id));
        lock.claim(id, secret);
    }

    function test_claimTwiceReverts() public {
        bytes32 id = _fill(keccak256("src-1"), 12e6);
        lock.claim(id, secret);
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.NotOpen.selector, id));
        lock.claim(id, secret);
    }

    function test_refundBeforeExpiryReverts() public {
        bytes32 id = _fill(keccak256("src-1"), 12e6);
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.NotExpired.selector, id));
        lock.refund(id);
    }

    function test_refundRestoresBobsAquaAllowance() public {
        bytes32 id = _fill(keccak256("src-1"), 12e6);
        vm.warp(block.timestamp + 10 minutes);
        vm.expectEmit(address(lock));
        emit DestinationLock.Refunded(id, bob, 12e6, true);
        vm.prank(makeAddr("anyone"));
        lock.refund(id);
        assertEq(usdt.balanceOf(bob), 100e6);
        assertEq(_allowance(), 50e6);
        assertEq(usdt.balanceOf(address(lock)), 0);
        assertEq(usdt.allowance(address(lock), address(aqua)), 0);
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.NotOpen.selector, id));
        lock.claim(id, secret);
    }

    function test_refundAfterDockPaysBobDirectly() public {
        bytes32 id = _fill(keccak256("src-1"), 12e6);
        address[] memory tokens = new address[](1);
        tokens[0] = address(usdt);
        vm.prank(bob);
        aqua.dock(address(pool), strategyHash, tokens);

        vm.warp(block.timestamp + 10 minutes);
        vm.expectEmit(address(lock));
        emit DestinationLock.Refunded(id, bob, 12e6, false);
        lock.refund(id);
        assertEq(usdt.balanceOf(bob), 100e6);
        assertEq(usdt.balanceOf(address(lock)), 0);
        assertEq(usdt.allowance(address(lock), address(aqua)), 0);
    }

    function test_fillEmitsFilled() public {
        bytes32 id = lock.lockIdFor(SRC_CHAIN, keccak256("src-1"));
        vm.expectEmit(address(pool));
        emit PayoutPool.Filled(keccak256("order-1002"), bob, strategyHash, 12e6, id);
        _fill(keccak256("src-1"), 12e6);
    }

    function testFuzz_onlyTheSecretOpensTheLock(bytes32 guess) public {
        vm.assume(guess != secret);
        bytes32 id = _fill(keccak256("src-1"), 12e6);
        vm.expectRevert(abi.encodeWithSelector(DestinationLock.WrongSecret.selector, id));
        lock.claim(id, guess);
    }
}
