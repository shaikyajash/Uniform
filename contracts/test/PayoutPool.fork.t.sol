// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Test} from "forge-std/Test.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {IAqua} from "@1inch/aqua/contracts/interfaces/IAqua.sol";
import {PayoutPool} from "../src/PayoutPool.sol";
import {DestinationLock} from "../src/DestinationLock.sol";

/// Runs PayoutPool and DestinationLock against the deployed Aqua and the real Whitechain Sepolia
/// USDT on a local fork. Nothing is sent. Run with: forge test --match-contract PayoutPoolForkTest
contract PayoutPoolForkTest is Test {
    IAqua constant AQUA = IAqua(0x02dF2659B5B4956EAFEe3293584C2aaefC114871);
    IERC20 constant USDT = IERC20(0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7);

    PayoutPool pool;
    DestinationLock lock;
    address bob = makeAddr("bob");
    address filler = makeAddr("matchmaker");
    address shop = makeAddr("shop");
    bytes32 secret = keccak256("fork-secret");
    PayoutPool.Strategy strategy;
    bytes32 strategyHash;

    function setUp() public {
        vm.createSelectFork("whitechain");
        pool = new PayoutPool(AQUA, USDT);
        lock = pool.destinationLock();
        deal(address(USDT), bob, 50e6);
        strategy = PayoutPool.Strategy({
            maker: bob, token: address(USDT), filler: filler, maxPerOrder: 20e6, feeBps: 30, salt: bytes32("fork")
        });
        vm.startPrank(bob);
        USDT.approve(address(AQUA), type(uint256).max);
        address[] memory tokens = new address[](1);
        tokens[0] = address(USDT);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 30e6;
        strategyHash = AQUA.ship(address(pool), abi.encode(strategy), tokens, amounts);
        vm.stopPrank();
    }

    function _fill() internal returns (bytes32) {
        vm.prank(filler);
        return pool.fill(
            strategy,
            PayoutPool.Fill({
                orderId: keccak256("order-4001"),
                merchant: shop,
                amount: 12e6,
                hashlock: keccak256(abi.encodePacked(secret)),
                expiry: uint64(block.timestamp + 10 minutes),
                srcChainId: 84532,
                srcLockId: keccak256("src-4001")
            })
        );
    }

    function test_fillAndClaimWithRealAquaAndUsdt() public {
        bytes32 id = _fill();
        assertEq(USDT.balanceOf(address(lock)), 12e6);
        assertEq(USDT.balanceOf(bob), 38e6);
        lock.claim(id, secret);
        assertEq(USDT.balanceOf(shop), 12e6);
    }

    function test_refundPushesBackIntoRealAqua() public {
        bytes32 id = _fill();
        vm.warp(block.timestamp + 10 minutes);
        lock.refund(id);
        assertEq(USDT.balanceOf(bob), 50e6);
        (uint248 allowance,) = AQUA.rawBalances(bob, address(pool), strategyHash, address(USDT));
        assertEq(allowance, 30e6);
    }
}
