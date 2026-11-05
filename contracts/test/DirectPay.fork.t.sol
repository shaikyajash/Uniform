// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {DirectPay} from "../src/DirectPay.sol";
import {Sign3009} from "./utils/Sign3009.sol";

/// Runs DirectPay against the real Whitechain Sepolia USDT (TetherTokenV2) on a local fork.
/// Nothing is sent to the network. Run with: forge test --match-contract DirectPayForkTest
contract DirectPayForkTest is Sign3009 {
    IERC20 constant USDT = IERC20(0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7);

    DirectPay directPay;
    uint256 customerKey = 0xA11CE;
    uint256 shopKey = 0x5409;
    address customer;
    address shop;

    function setUp() public {
        vm.createSelectFork("whitechain");
        customer = vm.addr(customerKey);
        shop = vm.addr(shopKey);
        directPay = new DirectPay(USDT);
        deal(address(USDT), customer, 50e6);
    }

    function test_domainSeparatorMatchesWalletDomain() public view {
        bytes32 expected = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("Tether USD"),
                keccak256("1"),
                uint256(1874),
                address(USDT)
            )
        );
        (bool ok, bytes memory data) = address(USDT).staticcall(abi.encodeWithSignature("DOMAIN_SEPARATOR()"));
        assertTrue(ok);
        assertEq(abi.decode(data, (bytes32)), expected);
    }

    function test_payAndRefundWithRealUsdt() public {
        bytes32 orderId = keccak256("order-2001");
        bytes32 paymentId = directPay.paymentIdFor(orderId, shop, 12e6, customer);
        Auth memory a = signReceive(
            address(USDT), customerKey, address(directPay), 12e6, block.timestamp - 1, block.timestamp + 300, paymentId
        );
        directPay.pay(orderId, shop, 12e6, customer, a.validAfter, a.validBefore, a.v, a.r, a.s);
        assertEq(USDT.balanceOf(shop), 12e6);
        assertEq(USDT.balanceOf(customer), 38e6);

        bytes32 nonce = directPay.refundNonceFor(paymentId, 4e6, 0);
        Auth memory r = signReceive(
            address(USDT), shopKey, address(directPay), 4e6, block.timestamp - 1, block.timestamp + 300, nonce
        );
        directPay.refund(paymentId, 4e6, r.validAfter, r.validBefore, r.v, r.r, r.s);
        assertEq(USDT.balanceOf(customer), 42e6);
        assertEq(USDT.balanceOf(shop), 8e6);
        assertEq(USDT.balanceOf(address(directPay)), 0);
    }

    function test_realUsdtRejectsReplay() public {
        bytes32 orderId = keccak256("order-2001");
        bytes32 paymentId = directPay.paymentIdFor(orderId, shop, 12e6, customer);
        Auth memory a = signReceive(
            address(USDT), customerKey, address(directPay), 12e6, block.timestamp - 1, block.timestamp + 300, paymentId
        );
        directPay.pay(orderId, shop, 12e6, customer, a.validAfter, a.validBefore, a.v, a.r, a.s);

        DirectPay second = new DirectPay(USDT);
        vm.expectRevert();
        second.pay(orderId, shop, 12e6, customer, a.validAfter, a.validBefore, a.v, a.r, a.s);
    }
}
