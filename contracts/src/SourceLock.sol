// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";

/// @notice EIP-3009 `receiveWithAuthorization`, as implemented by Circle's USDC.
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
/// @notice The source lock of a Uniform cross-chain payment. The customer's USDC is held for a
/// receiver (normally Bob, the liquidity provider) under a hashlock and an expiry:
///   - claim(lockId, secret) before expiry pays the receiver (source redeem);
///   - refund(lockId) at or after expiry returns the USDC to the payer.
/// Customers lock gaslessly with an EIP-3009 authorization whose nonce is the lock id:
///     lockId = keccak256(abi.encode(LOCK_TAG, chainid, this, params, payer, tokenIn, amountIn))
/// so a submitter cannot change the receiver, hashlock, expiry or amount, and each authorization
/// works once. Only USDC is accepted for now; swaps from other tokens are added separately.
contract SourceLock {
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
    }

    bytes32 public constant LOCK_TAG = keccak256("uniform.sourcelock.lock.v1");
    uint256 public constant MIN_LOCK_TIME = 5 minutes;
    uint256 public constant MAX_LOCK_TIME = 3 hours;
    uint256 public constant MAX_AUTH_WINDOW = 1 hours;

    IERC20 public immutable usdc;

    mapping(bytes32 lockId => Lock) public locks;

    event Locked(
        bytes32 indexed lockId,
        bytes32 indexed orderId,
        address payer,
        address receiver,
        uint256 usdcAmount,
        bytes32 hashlock,
        uint64 expiry
    );
    event Claimed(bytes32 indexed lockId, bytes32 secret);
    event Refunded(bytes32 indexed lockId);

    error ZeroToken();
    error ZeroAmount();
    error AmountTooLarge(uint256 amount);
    error ZeroReceiver();
    error ZeroHashlock();
    error ExpiryOutOfRange(uint64 expiry);
    error UnsupportedToken(address token);
    error SwapNotSupported();
    error BelowMinimum(uint256 amount, uint256 minUsdcOut);
    error AuthorizationWindowTooLong(uint256 validBefore, uint256 latestAllowed);
    error LockExists(bytes32 lockId);
    error NotOpen(bytes32 lockId);
    error LockExpired(bytes32 lockId);
    error NotExpired(bytes32 lockId);
    error WrongSecret(bytes32 lockId);

    constructor(IERC20 usdc_) {
        if (address(usdc_) == address(0)) revert ZeroToken();
        usdc = usdc_;
    }

    /// @notice The lock id, which is also the EIP-3009 nonce the customer signs.
    function lockIdFor(LockParams calldata p, address payer, address tokenIn, uint256 amountIn)
        public
        view
        returns (bytes32)
    {
        return keccak256(abi.encode(LOCK_TAG, block.chainid, address(this), p, payer, tokenIn, amountIn));
    }

    /// @notice Locks the payer's USDC using their signed authorization. Anyone may submit it.
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
    ) external returns (bytes32 lockId) {
        if (tokenIn != address(usdc)) revert UnsupportedToken(tokenIn);
        if (swapData.length != 0) revert SwapNotSupported();
        if (validBefore > block.timestamp + MAX_AUTH_WINDOW) {
            revert AuthorizationWindowTooLong(validBefore, block.timestamp + MAX_AUTH_WINDOW);
        }
        lockId = lockIdFor(p, payer, tokenIn, amountIn);
        _open(lockId, p, payer, amountIn);
        IERC3009(address(usdc))
            .receiveWithAuthorization(payer, address(this), amountIn, validAfter, validBefore, lockId, v, r, s);
    }

    /// @notice Locks the caller's USDC (approved beforehand). Used by the provider for refunds.
    function lockFromProvider(LockParams calldata p, uint256 usdcAmount) external returns (bytes32 lockId) {
        lockId = lockIdFor(p, msg.sender, address(usdc), usdcAmount);
        _open(lockId, p, msg.sender, usdcAmount);
        usdc.safeTransferFrom(msg.sender, address(this), usdcAmount);
    }

    /// @notice Source redeem: pays the receiver if `secret` matches the hashlock before expiry.
    function claim(bytes32 lockId, bytes32 secret) external {
        Lock storage l = locks[lockId];
        if (l.state != State.Open) revert NotOpen(lockId);
        if (block.timestamp >= l.expiry) revert LockExpired(lockId);
        if (keccak256(abi.encodePacked(secret)) != l.hashlock) revert WrongSecret(lockId);
        l.state = State.Claimed;
        usdc.safeTransfer(l.receiver, l.amount);
        emit Claimed(lockId, secret);
    }

    /// @notice Returns the USDC to the payer once the lock has expired. Anyone may call it.
    function refund(bytes32 lockId) external {
        Lock storage l = locks[lockId];
        if (l.state != State.Open) revert NotOpen(lockId);
        if (block.timestamp < l.expiry) revert NotExpired(lockId);
        l.state = State.Refunded;
        usdc.safeTransfer(l.payer, l.amount);
        emit Refunded(lockId);
    }

    function _open(bytes32 lockId, LockParams calldata p, address payer, uint256 amount) private {
        if (amount == 0) revert ZeroAmount();
        if (amount > type(uint96).max) revert AmountTooLarge(amount);
        if (amount < p.minUsdcOut) revert BelowMinimum(amount, p.minUsdcOut);
        if (p.receiver == address(0)) revert ZeroReceiver();
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
            orderId: p.orderId
        });
        emit Locked(lockId, p.orderId, payer, p.receiver, amount, p.hashlock, p.expiry);
    }
}
