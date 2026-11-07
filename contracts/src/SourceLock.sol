// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

/// @notice EIP-3009 `receiveWithAuthorization`, as implemented by Circle's USDC and EURC.
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

/// @title SourceLock
/// @notice The source lock of a Uniform cross-chain payment. The customer's tokens are held, as the
/// token they paid with, for a receiver (normally Bob, the liquidity provider) under a hashlock and
/// an expiry. Any ERC-20 that moves exact amounts can be locked:
///   - claim(lockId, secret) before expiry pays the receiver the locked token (source redeem);
///   - refund(lockId) at or after expiry returns exactly the locked token and amount to the payer.
/// The lock never swaps, so a payment that fails costs the customer no swap fee. The receiver
/// converts what it collects to USDC separately.
/// Each lock has the id
///     lockId = keccak256(abi.encode(LOCK_TAG, chainid, this, params, payer, tokenIn, amountIn))
/// and opens two ways:
///   - lockWithAuthorization: the note tokens fixed at deploy (USDC and EURC) with an EIP-3009
///     authorization whose nonce is the lock id, so a submitter cannot change the receiver,
///     hashlock, expiry, token or amount, and each authorization works once. An authorization may
///     live at most MAX_AUTH_WINDOW and must end at least MIN_TIME_AFTER_AUTH before the lock expires;
///   - lock: the caller locks its own approved tokens of any ERC-20 (a provider, or a customer's
///     smart account such as an EIP-7702 delegated wallet running a batch). The contract must
///     receive exactly the amount, so fee-on-transfer and rebasing tokens are refused.
/// Anyone can open a lock with any orderId and any token, so off-chain watchers must trust only the
/// exact lock id they expect, never an orderId match.
contract SourceLock is ReentrancyGuard {
    using SafeERC20 for IERC20;

    enum State {
        None,
        Open,
        Claimed,
        Refunded
    }

    struct LockParams {
        bytes32 orderId;
        address receiver;
        /// The USDC value the quote promised. Enforced on chain for USDC locks only; for other
        /// tokens the receiver checks the value before acting on the lock.
        uint256 minUsdcOut;
        bytes32 hashlock;
        uint64 expiry;
        address outToken;
    }

    struct Lock {
        address payer;
        uint64 expiry;
        State state;
        address receiver;
        uint96 amount;
        bytes32 hashlock;
        bytes32 orderId;
        address token;
    }

    bytes32 public constant LOCK_TAG = keccak256("uniform.sourcelock.lock.v2");
    uint256 public constant MIN_LOCK_TIME = 5 minutes;
    uint256 public constant MAX_LOCK_TIME = 3 hours;
    uint256 public constant MAX_AUTH_WINDOW = 1 hours;
    uint256 public constant MIN_TIME_AFTER_AUTH = 15 minutes;

    /// The note token whose amount is checked against `minUsdcOut`, and the only `outToken`.
    IERC20 public immutable usdc;
    /// The second note token (EURC).
    IERC20 public immutable altToken;

    mapping(bytes32 lockId => Lock) public locks;

    event Locked(
        bytes32 indexed lockId,
        bytes32 indexed orderId,
        address payer,
        address receiver,
        address token,
        uint256 amount,
        bytes32 hashlock,
        uint64 expiry
    );
    event Claimed(bytes32 indexed lockId, bytes32 secret);
    event Refunded(bytes32 indexed lockId);

    error ZeroToken();
    error ZeroAmount();
    error AmountTooLarge(uint256 amount);
    error ZeroReceiver();
    error InvalidReceiver(address receiver);
    error ZeroHashlock();
    error ExpiryOutOfRange(uint64 expiry);
    error UnsupportedToken(address token);
    error SwapNotSupported();
    error BelowMinimum(uint256 amount, uint256 minUsdcOut);
    error AuthorizationWindowTooLong(uint256 validAfter, uint256 validBefore);
    error AuthorizationTooLate(uint256 validBefore, uint64 expiry);
    error LockExists(bytes32 lockId);
    error NotOpen(bytes32 lockId);
    error LockExpired(bytes32 lockId);
    error NotExpired(bytes32 lockId);
    error WrongSecret(bytes32 lockId);
    error AmountNotReceived(uint256 expected, uint256 received);

    constructor(IERC20 usdc_, IERC20 altToken_) {
        if (address(usdc_) == address(0) || address(altToken_) == address(0) || address(usdc_) == address(altToken_)) {
            revert ZeroToken();
        }
        usdc = usdc_;
        altToken = altToken_;
    }

    /// @notice True for the tokens `lockWithAuthorization` accepts (USDC and EURC).
    function isNoteToken(address token) public view returns (bool) {
        return token == address(usdc) || token == address(altToken);
    }

    /// @notice The lock id, which is also the EIP-3009 nonce the customer signs.
    function lockIdFor(LockParams calldata p, address payer, address tokenIn, uint256 amountIn)
        public
        view
        returns (bytes32)
    {
        return keccak256(abi.encode(LOCK_TAG, block.chainid, address(this), p, payer, tokenIn, amountIn));
    }

    /// @notice Locks the payer's USDC or EURC using their signed authorization. Anyone may submit it.
    function lockWithAuthorization(
        LockParams calldata p,
        address payer,
        address tokenIn,
        uint256 amountIn,
        uint256 validAfter,
        uint256 validBefore,
        uint8 v,
        bytes32 r,
        bytes32 s,
        bytes calldata swapData
    ) external nonReentrant returns (bytes32 lockId) {
        if (swapData.length != 0) revert SwapNotSupported();
        if (validBefore > validAfter + MAX_AUTH_WINDOW) revert AuthorizationWindowTooLong(validAfter, validBefore);
        if (validBefore + MIN_TIME_AFTER_AUTH > p.expiry) revert AuthorizationTooLate(validBefore, p.expiry);
        if (!isNoteToken(tokenIn)) revert UnsupportedToken(tokenIn);
        lockId = lockIdFor(p, payer, tokenIn, amountIn);
        _open(lockId, p, payer, tokenIn, amountIn);
        IERC3009(tokenIn)
            .receiveWithAuthorization(payer, address(this), amountIn, validAfter, validBefore, lockId, v, r, s);
    }

    /// @notice Locks the caller's own tokens of any ERC-20 (approved beforehand). The caller is the
    /// payer: a provider locking for a refund, or a customer's account paying directly. Reverts
    /// unless this contract's balance of `token` rises by exactly `amount`.
    function lock(LockParams calldata p, address token, uint256 amount) external nonReentrant returns (bytes32 lockId) {
        lockId = lockIdFor(p, msg.sender, token, amount);
        _open(lockId, p, msg.sender, token, amount);
        uint256 before = IERC20(token).balanceOf(address(this));
        IERC20(token).safeTransferFrom(msg.sender, address(this), amount);
        uint256 balance = IERC20(token).balanceOf(address(this));
        if (balance != before + amount) revert AmountNotReceived(amount, balance > before ? balance - before : 0);
    }

    /// @notice Source redeem: pays the receiver the locked token if `secret` matches before expiry.
    function claim(bytes32 lockId, bytes32 secret) external nonReentrant {
        Lock storage l = locks[lockId];
        if (l.state != State.Open) revert NotOpen(lockId);
        if (block.timestamp >= l.expiry) revert LockExpired(lockId);
        if (keccak256(abi.encodePacked(secret)) != l.hashlock) revert WrongSecret(lockId);
        l.state = State.Claimed;
        IERC20(l.token).safeTransfer(l.receiver, l.amount);
        emit Claimed(lockId, secret);
    }

    /// @notice Returns exactly the locked token and amount to the payer once expired. Anyone may call it.
    function refund(bytes32 lockId) external nonReentrant {
        Lock storage l = locks[lockId];
        if (l.state != State.Open) revert NotOpen(lockId);
        if (block.timestamp < l.expiry) revert NotExpired(lockId);
        l.state = State.Refunded;
        IERC20(l.token).safeTransfer(l.payer, l.amount);
        emit Refunded(lockId);
    }

    function _open(bytes32 lockId, LockParams calldata p, address payer, address token, uint256 amount) private {
        if (token == address(0)) revert ZeroToken();
        if (token.code.length == 0) revert UnsupportedToken(token);
        if (amount == 0) revert ZeroAmount();
        if (amount > type(uint96).max) revert AmountTooLarge(amount);
        if (token == address(usdc) && amount < p.minUsdcOut) revert BelowMinimum(amount, p.minUsdcOut);
        if (p.receiver == address(0)) revert ZeroReceiver();
        if (p.receiver == address(this) || p.receiver == token) revert InvalidReceiver(p.receiver);
        if (p.hashlock == bytes32(0)) revert ZeroHashlock();
        if (p.expiry < block.timestamp + MIN_LOCK_TIME || p.expiry > block.timestamp + MAX_LOCK_TIME) {
            revert ExpiryOutOfRange(p.expiry);
        }
        if (p.outToken != address(0) && p.outToken != address(usdc)) revert UnsupportedToken(p.outToken);
        if (locks[lockId].state != State.None) revert LockExists(lockId);

        locks[lockId] = Lock({
            payer: payer,
            expiry: p.expiry,
            state: State.Open,
            receiver: p.receiver,
            amount: uint96(amount),
            hashlock: p.hashlock,
            orderId: p.orderId,
            token: token
        });
        emit Locked(lockId, p.orderId, payer, p.receiver, token, amount, p.hashlock, p.expiry);
    }
}
