// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Script, console} from "forge-std/Script.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {DirectPay} from "../src/DirectPay.sol";

interface IDomain {
    function DOMAIN_SEPARATOR() external view returns (bytes32);
}

/// End-to-end check of DirectPay on Whitechain Sepolia with test wallets:
/// the customer signs a payment to the shop, the shop signs a refund of the same amount,
/// and the gas payer submits both. Reads CUSTOMER_, SHOP_ and GAS_PAYER_PRIVATE_KEY from the env.
contract TryDirectPay is Script {
    IERC20 constant USDT = IERC20(0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7);
    DirectPay constant DIRECT_PAY = DirectPay(0x9DF1C7C32Ce3f1322e1c7bAEc30664b1c472DaE9);
    bytes32 constant RECEIVE_TYPEHASH = keccak256(
        "ReceiveWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)"
    );

    function _sign(uint256 key, uint256 value, uint256 validAfter, uint256 validBefore, bytes32 nonce)
        internal
        view
        returns (uint8 v, bytes32 r, bytes32 s)
    {
        bytes32 structHash = keccak256(
            abi.encode(RECEIVE_TYPEHASH, vm.addr(key), address(DIRECT_PAY), value, validAfter, validBefore, nonce)
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", IDomain(address(USDT)).DOMAIN_SEPARATOR(), structHash));
        (v, r, s) = vm.sign(key, digest);
    }

    function run() external {
        require(block.chainid == 1874, "not Whitechain Sepolia");
        uint256 customerKey = vm.envUint("CUSTOMER_PRIVATE_KEY");
        uint256 shopKey = vm.envUint("SHOP_PRIVATE_KEY");
        uint256 gasPayerKey = vm.envUint("GAS_PAYER_PRIVATE_KEY");
        address customer = vm.addr(customerKey);
        address shop = vm.addr(shopKey);
        uint256 amount = vm.envOr("AMOUNT", uint256(1e6));
        bytes32 orderId = keccak256(abi.encode("uniform-test-order", block.timestamp));

        uint256 validAfter = block.timestamp - 60;
        uint256 validBefore = block.timestamp + 600;
        bytes32 paymentId = DIRECT_PAY.paymentIdFor(orderId, shop, amount, customer);
        (uint8 pv, bytes32 pr, bytes32 ps) = _sign(customerKey, amount, validAfter, validBefore, paymentId);
        bytes32 refundNonce = DIRECT_PAY.refundNonceFor(paymentId, amount, 0);
        (uint8 rv, bytes32 rr, bytes32 rs) = _sign(shopKey, amount, validAfter, validBefore, refundNonce);

        console.log("customer USDT before:", USDT.balanceOf(customer));
        vm.startBroadcast(gasPayerKey);
        DIRECT_PAY.pay(orderId, shop, amount, customer, validAfter, validBefore, pv, pr, ps);
        DIRECT_PAY.refund(paymentId, amount, validAfter, validBefore, rv, rr, rs);
        vm.stopBroadcast();
        console.log("paymentId:");
        console.logBytes32(paymentId);
        console.log("customer USDT after:", USDT.balanceOf(customer));
        console.log("shop USDT after:", USDT.balanceOf(shop));
    }
}
