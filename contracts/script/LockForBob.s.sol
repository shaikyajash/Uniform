// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Script, console} from "forge-std/Script.sol";
import {SourceLock} from "../src/SourceLock.sol";

interface IDomain {
    function DOMAIN_SEPARATOR() external view returns (bytes32);
}

/// Plays the customer for a cross-chain test on Base Sepolia or Ethereum Sepolia: the customer signs a 20-minute
/// source lock of their USDC for Bob, and the gas payer submits it. The Matchmaker then fills the
/// destination lock; reveal SECRET on the DestinationLock to pay the shop.
/// Env: SOURCE_LOCK, CUSTOMER_PRIVATE_KEY, GAS_PAYER_PRIVATE_KEY, BOB_ADDRESS, ORDER_ID, SECRET,
/// AMOUNT (smallest units of the token), optional TOKEN (default USDC; EURC is also allowed) and
/// MIN_USDC_OUT (the quote's USDC value; default AMOUNT).
contract LockForBob is Script {
    bytes32 constant RECEIVE_TYPEHASH = keccak256(
        "ReceiveWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)"
    );

    function run() external {
        address usdc;
        if (block.chainid == 84532) usdc = 0x036CbD53842c5426634e7929541eC2318f3dCF7e;
        else if (block.chainid == 11155111) usdc = 0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238;
        else revert("not Base Sepolia or Ethereum Sepolia");
        SourceLock lock = SourceLock(vm.envAddress("SOURCE_LOCK"));
        uint256 customerKey = vm.envUint("CUSTOMER_PRIVATE_KEY");
        address customer = vm.addr(customerKey);
        uint256 amount = vm.envUint("AMOUNT");
        address token = vm.envOr("TOKEN", usdc);
        bytes32 secret = vm.envBytes32("SECRET");

        SourceLock.LockParams memory p = SourceLock.LockParams({
            orderId: vm.envBytes32("ORDER_ID"),
            receiver: vm.envAddress("BOB_ADDRESS"),
            minUsdcOut: vm.envOr("MIN_USDC_OUT", amount),
            hashlock: keccak256(abi.encodePacked(secret)),
            expiry: uint64(block.timestamp + 20 minutes),
            outToken: address(0)
        });
        uint256 validAfter = block.timestamp - 60;
        uint256 validBefore = block.timestamp + 300;
        bytes32 nonce = lock.lockIdFor(p, customer, token, amount);
        bytes32 digest = keccak256(
            abi.encodePacked(
                "\x19\x01",
                IDomain(token).DOMAIN_SEPARATOR(),
                keccak256(abi.encode(RECEIVE_TYPEHASH, customer, address(lock), amount, validAfter, validBefore, nonce))
            )
        );
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(customerKey, digest);

        vm.startBroadcast(vm.envUint("GAS_PAYER_PRIVATE_KEY"));
        bytes32 lockId =
            lock.lockWithAuthorization(p, customer, token, amount, validAfter, validBefore, v, r, s, "");
        vm.stopBroadcast();
        console.log("source lockId:");
        console.logBytes32(lockId);
    }
}
