// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Script, console} from "forge-std/Script.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {IAqua} from "@1inch/aqua/contracts/interfaces/IAqua.sol";
import {PayoutPool} from "../src/PayoutPool.sol";
import {DestinationLock} from "../src/DestinationLock.sol";

/// End-to-end check on Whitechain Sepolia with test wallets: Bob approves Aqua and ships a strategy
/// to the PayoutPool, the filler fills a destination lock for the shop, then the secret redeems it.
/// Env: PAYOUT_POOL, BOB_PRIVATE_KEY, GAS_PAYER_PRIVATE_KEY (filler and redeemer), SHOP_PRIVATE_KEY
/// (merchant), optional AMOUNT (default 1 USDT).
contract TryPayoutPool is Script {
    IAqua constant AQUA = IAqua(0x02dF2659B5B4956EAFEe3293584C2aaefC114871);
    IERC20 constant USDT = IERC20(0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7);

    function run() external {
        require(block.chainid == 1874, "not Whitechain Sepolia");
        PayoutPool pool = PayoutPool(vm.envAddress("PAYOUT_POOL"));
        DestinationLock lock = pool.destinationLock();
        uint256 bobKey = vm.envUint("BOB_PRIVATE_KEY");
        uint256 gasPayerKey = vm.envUint("GAS_PAYER_PRIVATE_KEY");
        address bob = vm.addr(bobKey);
        address shop = vm.addr(vm.envUint("SHOP_PRIVATE_KEY"));
        uint256 amount = vm.envOr("AMOUNT", uint256(1e6));

        PayoutPool.Strategy memory s = PayoutPool.Strategy({
            maker: bob,
            token: address(USDT),
            filler: vm.addr(gasPayerKey),
            maxPerOrder: uint128(amount),
            feeBps: 30,
            salt: keccak256(abi.encode("uniform-test-strategy", block.timestamp))
        });
        bytes32 secret = keccak256(abi.encode("uniform-test-secret", block.timestamp));
        PayoutPool.Fill memory f = PayoutPool.Fill({
            orderId: keccak256(abi.encode("uniform-test-order", block.timestamp)),
            merchant: shop,
            amount: amount,
            hashlock: keccak256(abi.encodePacked(secret)),
            expiry: uint64(block.timestamp + 10 minutes),
            srcChainId: 84532,
            srcLockId: keccak256(abi.encode("uniform-test-src-lock", block.timestamp))
        });
        address[] memory tokens = new address[](1);
        tokens[0] = address(USDT);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = amount;

        console.log("shop USDT before:", USDT.balanceOf(shop));
        vm.startBroadcast(bobKey);
        if (USDT.allowance(bob, address(AQUA)) < amount) USDT.approve(address(AQUA), type(uint256).max);
        AQUA.ship(address(pool), abi.encode(s), tokens, amounts);
        vm.stopBroadcast();

        vm.startBroadcast(gasPayerKey);
        bytes32 lockId = pool.fill(s, f);
        lock.claim(lockId, secret);
        vm.stopBroadcast();

        console.log("lockId:");
        console.logBytes32(lockId);
        console.log("shop USDT after:", USDT.balanceOf(shop));
    }
}
