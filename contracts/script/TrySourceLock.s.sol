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
        uint256 validBefore = block.timestamp + 300;
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
