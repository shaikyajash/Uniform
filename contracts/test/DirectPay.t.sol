// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {DirectPay} from "../src/DirectPay.sol";
import {MockEIP3009} from "./mocks/MockEIP3009.sol";
import {Sign3009} from "./utils/Sign3009.sol";

contract DirectPayTest is Sign3009 {
    MockEIP3009 usdt;
    DirectPay directPay;

    uint256 customerKey = 0xA11CE;
    uint256 shopKey = 0x5409;
    address customer;
    address shop;
    address relayer = makeAddr("gasPayer");

    bytes32 constant ORDER = keccak256("order-2001");
    uint256 constant PRICE = 12e6;

    event Paid(
        bytes32 indexed paymentId, bytes32 indexed orderId, address indexed merchant, address payer, uint256 amount
    );
    event Refunded(
        bytes32 indexed paymentId, bytes32 indexed orderId, address indexed payer, uint256 amount, uint256 totalRefunded
    );

    function setUp() public {
        vm.warp(1_800_000_000);
        customer = vm.addr(customerKey);
        shop = vm.addr(shopKey);
        usdt = new MockEIP3009();
        directPay = new DirectPay(IERC20(address(usdt)));
        usdt.mint(customer, 50e6);
    }

    function _payAuth(bytes32 orderId, address merchant, uint256 amount) internal view returns (Auth memory) {
        bytes32 nonce = directPay.paymentIdFor(orderId, merchant, amount, customer);
        return signReceive(
            address(usdt), customerKey, address(directPay), amount, block.timestamp - 1, block.timestamp + 300, nonce
        );
    }

    function _pay(bytes32 orderId, address merchant, uint256 amount, Auth memory a) internal returns (bytes32) {
        return directPay.pay(orderId, merchant, amount, customer, a.validAfter, a.validBefore, a.v, a.r, a.s);
    }

    function _payDefault() internal returns (bytes32 paymentId) {
        vm.prank(relayer);
        paymentId = _pay(ORDER, shop, PRICE, _payAuth(ORDER, shop, PRICE));
    }

    function _refundAuth(bytes32 paymentId, uint256 amount, uint256 refundedBefore)
        internal
        view
        returns (Auth memory)
    {
        bytes32 nonce = directPay.refundNonceFor(paymentId, amount, refundedBefore);
        return signReceive(
            address(usdt), shopKey, address(directPay), amount, block.timestamp - 1, block.timestamp + 300, nonce
        );
    }

    // ---- paying ----

    function test_payMovesUsdtFromCustomerToShop() public {
        bytes32 expectedId = directPay.paymentIdFor(ORDER, shop, PRICE, customer);
        Auth memory a = _payAuth(ORDER, shop, PRICE);

        vm.expectEmit(address(directPay));
        emit Paid(expectedId, ORDER, shop, customer, PRICE);
        vm.prank(relayer);
        bytes32 paymentId = _pay(ORDER, shop, PRICE, a);

        assertEq(paymentId, expectedId);
        assertEq(usdt.balanceOf(customer), 38e6);
        assertEq(usdt.balanceOf(shop), PRICE);
        assertEq(usdt.balanceOf(address(directPay)), 0);
        assertEq(usdt.balanceOf(relayer), 0);
        (address payer, uint96 amount, address merchant, uint96 refunded, bytes32 orderId) =
            directPay.payments(paymentId);
        assertEq(payer, customer);
        assertEq(amount, PRICE);
        assertEq(merchant, shop);
        assertEq(refunded, 0);
        assertEq(orderId, ORDER);
    }

    function test_signatureCannotBeUsedTwice() public {
        Auth memory a = _payAuth(ORDER, shop, PRICE);
        _pay(ORDER, shop, PRICE, a);
        vm.expectRevert(abi.encodeWithSelector(DirectPay.PaymentAlreadyRecorded.selector, a_id()));
        _pay(ORDER, shop, PRICE, a);
    }

    function a_id() internal view returns (bytes32) {
        return directPay.paymentIdFor(ORDER, shop, PRICE, customer);
    }

    function test_cannotRedirectToAnotherShop() public {
        Auth memory a = _payAuth(ORDER, shop, PRICE);
        vm.expectRevert("invalid signature");
        _pay(ORDER, makeAddr("attacker"), PRICE, a);
    }

    function test_cannotChangeAmount() public {
        Auth memory a = _payAuth(ORDER, shop, PRICE);
        vm.expectRevert("invalid signature");
        _pay(ORDER, shop, PRICE + 1, a);
    }

    function test_cannotReuseForAnotherOrder() public {
        Auth memory a = _payAuth(ORDER, shop, PRICE);
        vm.expectRevert("invalid signature");
        _pay(keccak256("order-2002"), shop, PRICE, a);
    }

    function test_cannotChargeSomeoneElse() public {
        Auth memory a = _payAuth(ORDER, shop, PRICE);
        address victim = makeAddr("victim");
        usdt.mint(victim, 50e6);
        vm.expectRevert("invalid signature");
        directPay.pay(ORDER, shop, PRICE, victim, a.validAfter, a.validBefore, a.v, a.r, a.s);
    }

    function test_expiredAuthorizationReverts() public {
        Auth memory a = _payAuth(ORDER, shop, PRICE);
        vm.warp(a.validBefore);
        vm.expectRevert("expired");
        _pay(ORDER, shop, PRICE, a);
    }

    function test_notYetValidAuthorizationReverts() public {
        bytes32 nonce = directPay.paymentIdFor(ORDER, shop, PRICE, customer);
        Auth memory a = signReceive(
            address(usdt), customerKey, address(directPay), PRICE, block.timestamp + 10, block.timestamp + 300, nonce
        );
        vm.expectRevert("not yet valid");
        _pay(ORDER, shop, PRICE, a);
    }

    function test_rejectsZeroAmountAndZeroShop() public {
        Auth memory a = _payAuth(ORDER, shop, PRICE);
        vm.expectRevert(DirectPay.ZeroAmount.selector);
        _pay(ORDER, shop, 0, a);
        vm.expectRevert(DirectPay.ZeroMerchant.selector);
        _pay(ORDER, address(0), PRICE, a);
    }

    function test_insufficientBalanceRevertsAndRecordsNothing() public {
        uint256 tooMuch = 60e6;
        Auth memory a = _payAuth(ORDER, shop, tooMuch);
        vm.expectRevert();
        _pay(ORDER, shop, tooMuch, a);
        (address payer,,,,) = directPay.payments(directPay.paymentIdFor(ORDER, shop, tooMuch, customer));
        assertEq(payer, address(0));
    }

    function test_paymentIdIsBoundToThisContractAndChain() public {
        DirectPay other = new DirectPay(IERC20(address(usdt)));
        assertTrue(
            other.paymentIdFor(ORDER, shop, PRICE, customer) != directPay.paymentIdFor(ORDER, shop, PRICE, customer)
        );
    }

    // ---- refunds ----

    function test_partialRefundsGoToOriginalPayer() public {
        bytes32 paymentId = _payDefault();

        Auth memory r1 = _refundAuth(paymentId, 4e6, 0);
        vm.expectEmit(address(directPay));
        emit Refunded(paymentId, ORDER, customer, 4e6, 4e6);
        vm.prank(relayer);
        directPay.refund(paymentId, 4e6, r1.validAfter, r1.validBefore, r1.v, r1.r, r1.s);

        Auth memory r2 = _refundAuth(paymentId, 8e6, 4e6);
        directPay.refund(paymentId, 8e6, r2.validAfter, r2.validBefore, r2.v, r2.r, r2.s);

        assertEq(usdt.balanceOf(customer), 50e6);
        assertEq(usdt.balanceOf(shop), 0);
        (,,, uint96 refunded,) = directPay.payments(paymentId);
        assertEq(refunded, PRICE);
    }

    function test_refundCannotExceedPayment() public {
        bytes32 paymentId = _payDefault();
        Auth memory r = _refundAuth(paymentId, PRICE + 1, 0);
        vm.expectRevert(abi.encodeWithSelector(DirectPay.RefundExceedsPayment.selector, paymentId, PRICE + 1, PRICE));
        directPay.refund(paymentId, PRICE + 1, r.validAfter, r.validBefore, r.v, r.r, r.s);
    }

    function test_refundSignatureCannotBeReplayed() public {
        bytes32 paymentId = _payDefault();
        Auth memory r = _refundAuth(paymentId, 4e6, 0);
        directPay.refund(paymentId, 4e6, r.validAfter, r.validBefore, r.v, r.r, r.s);
        vm.expectRevert("invalid signature");
        directPay.refund(paymentId, 4e6, r.validAfter, r.validBefore, r.v, r.r, r.s);
    }

    function test_refundMustBeSignedByTheShop() public {
        bytes32 paymentId = _payDefault();
        bytes32 nonce = directPay.refundNonceFor(paymentId, 4e6, 0);
        Auth memory byCustomer = signReceive(
            address(usdt), customerKey, address(directPay), 4e6, block.timestamp - 1, block.timestamp + 300, nonce
        );
        vm.expectRevert("invalid signature");
        directPay.refund(
            paymentId, 4e6, byCustomer.validAfter, byCustomer.validBefore, byCustomer.v, byCustomer.r, byCustomer.s
        );
    }

    function test_refundForUnknownPaymentReverts() public {
        bytes32 unknown = keccak256("nope");
        Auth memory r = _refundAuth(unknown, 1e6, 0);
        vm.expectRevert(abi.encodeWithSelector(DirectPay.UnknownPayment.selector, unknown));
        directPay.refund(unknown, 1e6, r.validAfter, r.validBefore, r.v, r.r, r.s);
    }

    function test_zeroRefundReverts() public {
        bytes32 paymentId = _payDefault();
        Auth memory r = _refundAuth(paymentId, 0, 0);
        vm.expectRevert(DirectPay.ZeroAmount.selector);
        directPay.refund(paymentId, 0, r.validAfter, r.validBefore, r.v, r.r, r.s);
    }

    function testFuzz_payThenRefundAll(uint96 amount) public {
        amount = uint96(bound(amount, 1, 50e6));
        Auth memory a = _payAuth(ORDER, shop, amount);
        bytes32 paymentId = _pay(ORDER, shop, amount, a);
        assertEq(usdt.balanceOf(shop), amount);
        Auth memory r = _refundAuth(paymentId, amount, 0);
        directPay.refund(paymentId, amount, r.validAfter, r.validBefore, r.v, r.r, r.s);
        assertEq(usdt.balanceOf(customer), 50e6);
    }

    // ---- review fixes ----

    function test_thirdPartyCannotSquatTheCustomersPayment() public {
        uint256 attackerKey = 0xBAD;
        address attacker = vm.addr(attackerKey);
        usdt.mint(attacker, 50e6);
        bytes32 customersId = directPay.paymentIdFor(ORDER, shop, PRICE, customer);
        Auth memory squat = signReceive(
            address(usdt),
            attackerKey,
            address(directPay),
            PRICE,
            block.timestamp - 1,
            block.timestamp + 300,
            customersId
        );
        vm.expectRevert("invalid signature");
        directPay.pay(ORDER, shop, PRICE, attacker, squat.validAfter, squat.validBefore, squat.v, squat.r, squat.s);

        bytes32 paymentId = _payDefault();
        assertEq(paymentId, customersId);
        (address payer,,,,) = directPay.payments(paymentId);
        assertEq(payer, customer);
    }

    function test_authorizationWindowIsCapped() public {
        bytes32 nonce = directPay.paymentIdFor(ORDER, shop, PRICE, customer);
        uint256 tooLate = block.timestamp + directPay.MAX_AUTH_WINDOW() + 1;
        Auth memory a =
            signReceive(address(usdt), customerKey, address(directPay), PRICE, block.timestamp - 1, tooLate, nonce);
        vm.expectRevert(
            abi.encodeWithSelector(
                DirectPay.AuthorizationWindowTooLong.selector, tooLate, block.timestamp + directPay.MAX_AUTH_WINDOW()
            )
        );
        _pay(ORDER, shop, PRICE, a);
    }

    function test_rejectsContractOrTokenAsShop() public {
        Auth memory a = _payAuth(ORDER, shop, PRICE);
        vm.expectRevert(abi.encodeWithSelector(DirectPay.InvalidMerchant.selector, address(directPay)));
        _pay(ORDER, address(directPay), PRICE, a);
        vm.expectRevert(abi.encodeWithSelector(DirectPay.InvalidMerchant.selector, address(usdt)));
        _pay(ORDER, address(usdt), PRICE, a);
    }

    function test_rejectsZeroToken() public {
        vm.expectRevert(DirectPay.ZeroToken.selector);
        new DirectPay(IERC20(address(0)));
    }

    function test_staleRefundSignatureStopsWorking() public {
        bytes32 paymentId = _payDefault();
        Auth memory signedAtZero = _refundAuth(paymentId, 4e6, 0);
        Auth memory first = _refundAuth(paymentId, 2e6, 0);
        directPay.refund(paymentId, 2e6, first.validAfter, first.validBefore, first.v, first.r, first.s);
        vm.expectRevert("invalid signature");
        directPay.refund(
            paymentId,
            4e6,
            signedAtZero.validAfter,
            signedAtZero.validBefore,
            signedAtZero.v,
            signedAtZero.r,
            signedAtZero.s
        );
    }

    function testFuzz_manyPartialRefundsNeverExceedPayment(uint8 parts) public {
        parts = uint8(bound(parts, 1, 12));
        bytes32 paymentId = _payDefault();
        uint256 slice = PRICE / parts;
        uint256 done;
        for (uint256 i; i < parts; ++i) {
            uint256 amt = i == parts - 1 ? PRICE - done : slice;
            Auth memory r = _refundAuth(paymentId, amt, done);
            directPay.refund(paymentId, amt, r.validAfter, r.validBefore, r.v, r.r, r.s);
            done += amt;
        }
        assertEq(done, PRICE);
        assertEq(usdt.balanceOf(customer), 50e6);
        Auth memory extra = _refundAuth(paymentId, 1, done);
        vm.expectRevert(abi.encodeWithSelector(DirectPay.RefundExceedsPayment.selector, paymentId, 1, 0));
        directPay.refund(paymentId, 1, extra.validAfter, extra.validBefore, extra.v, extra.r, extra.s);
    }
}
