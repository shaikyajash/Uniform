// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";

/// @notice EIP-3009 `receiveWithAuthorization`, as implemented by Tether's TetherTokenV2.
interface IERC3009 {
    function receiveWithAuthorization(
        address from,
        address to,
        uint256 value,
        uint256 validAfter,
        uint256 validBefore,
        bytes32 nonce,
        uint8 v,
        bytes32 r,
        bytes32 s
    ) external;
}

/// @title DirectPay
/// @notice Gasless USDT checkout on one chain. A customer signs an EIP-3009 authorization and
/// anyone (normally the Uniform Gas Payer) submits it. The USDT goes to the shop and the payment is
/// recorded under a payment id, which is also the authorization nonce:
///     paymentId = keccak256(abi.encode(PAY_TAG, chainid, this, orderId, merchant, amount, payer))
/// Because the nonce commits to the order, shop, amount and payer, a submitter cannot redirect or
/// resize the payment, nobody else can record a payment under the customer's payment id, and each
/// authorization can be used only once. Authorizations may be valid for at most MAX_AUTH_WINDOW.
/// Refunds work the same way: the shop signs an authorization whose nonce commits to the payment,
/// the refund amount and the amount refunded so far, and the USDT goes back to the original payer.
contract DirectPay {
    using SafeERC20 for IERC20;

    struct Payment {
        address payer;
        uint96 amount;
        address merchant;
        uint96 refunded;
        bytes32 orderId;
    }

    bytes32 public constant PAY_TAG = keccak256("uniform.directpay.pay.v1");
    bytes32 public constant REFUND_TAG = keccak256("uniform.directpay.refund.v1");
    uint256 public constant MAX_AUTH_WINDOW = 1 hours;

    IERC20 public immutable token;

    mapping(bytes32 paymentId => Payment) public payments;

    event Paid(
        bytes32 indexed paymentId, bytes32 indexed orderId, address indexed merchant, address payer, uint256 amount
    );
    event Refunded(
        bytes32 indexed paymentId, bytes32 indexed orderId, address indexed payer, uint256 amount, uint256 totalRefunded
    );

    error ZeroAmount();
    error ZeroMerchant();
    error InvalidMerchant(address merchant);
    error ZeroToken();
    error AuthorizationWindowTooLong(uint256 validBefore, uint256 latestAllowed);
    error AmountTooLarge(uint256 amount);
    error PaymentAlreadyRecorded(bytes32 paymentId);
    error UnknownPayment(bytes32 paymentId);
    error RefundExceedsPayment(bytes32 paymentId, uint256 requested, uint256 refundable);

    constructor(IERC20 token_) {
        if (address(token_) == address(0)) revert ZeroToken();
        token = token_;
    }

    /// @notice The payment id and EIP-3009 nonce the customer signs for an order.
    function paymentIdFor(bytes32 orderId, address merchant, uint256 amount, address payer)
        public
        view
        returns (bytes32)
    {
        return keccak256(abi.encode(PAY_TAG, block.chainid, address(this), orderId, merchant, amount, payer));
    }

    /// @notice The EIP-3009 nonce the shop signs for a refund, given the amount refunded so far.
    function refundNonceFor(bytes32 paymentId, uint256 amount, uint256 refundedBefore) public view returns (bytes32) {
        return keccak256(abi.encode(REFUND_TAG, block.chainid, address(this), paymentId, amount, refundedBefore));
    }

    /// @notice Pays `merchant` `amount` USDT from `payer` using the payer's signed authorization.
    /// The authorization must be `receiveWithAuthorization(payer, this, amount, ..., nonce = paymentId)`.
    function pay(
        bytes32 orderId,
        address merchant,
        uint256 amount,
        address payer,
        uint256 validAfter,
        uint256 validBefore,
        uint8 v,
        bytes32 r,
        bytes32 s
    ) external returns (bytes32 paymentId) {
        if (amount == 0) revert ZeroAmount();
        if (merchant == address(0)) revert ZeroMerchant();
        if (merchant == address(this) || merchant == address(token)) revert InvalidMerchant(merchant);
        if (amount > type(uint96).max) revert AmountTooLarge(amount);
        _checkWindow(validBefore);

        paymentId = paymentIdFor(orderId, merchant, amount, payer);
        if (payments[paymentId].payer != address(0)) revert PaymentAlreadyRecorded(paymentId);
        payments[paymentId] =
            Payment({payer: payer, amount: uint96(amount), merchant: merchant, refunded: 0, orderId: orderId});

        IERC3009(address(token))
            .receiveWithAuthorization(payer, address(this), amount, validAfter, validBefore, paymentId, v, r, s);
        token.safeTransfer(merchant, amount);

        emit Paid(paymentId, orderId, merchant, payer, amount);
    }

    /// @notice Refunds `amount` of a payment to its original payer, using the shop's signed authorization.
    /// The authorization must be `receiveWithAuthorization(merchant, this, amount, ...,
    /// nonce = refundNonceFor(paymentId, amount, refundedSoFar))`.
    function refund(
        bytes32 paymentId,
        uint256 amount,
        uint256 validAfter,
        uint256 validBefore,
        uint8 v,
        bytes32 r,
        bytes32 s
    ) external {
        Payment storage p = payments[paymentId];
        if (p.payer == address(0)) revert UnknownPayment(paymentId);
        if (amount == 0) revert ZeroAmount();
        uint256 refundedBefore = p.refunded;
        uint256 refundable = uint256(p.amount) - refundedBefore;
        if (amount > refundable) revert RefundExceedsPayment(paymentId, amount, refundable);
        _checkWindow(validBefore);

        bytes32 nonce = refundNonceFor(paymentId, amount, refundedBefore);
        p.refunded = uint96(refundedBefore + amount);
        address payer = p.payer;

        IERC3009(address(token))
            .receiveWithAuthorization(p.merchant, address(this), amount, validAfter, validBefore, nonce, v, r, s);
        token.safeTransfer(payer, amount);

        emit Refunded(paymentId, p.orderId, payer, amount, refundedBefore + amount);
    }

    function _checkWindow(uint256 validBefore) private view {
        uint256 latestAllowed = block.timestamp + MAX_AUTH_WINDOW;
        if (validBefore > latestAllowed) revert AuthorizationWindowTooLong(validBefore, latestAllowed);
    }
}
