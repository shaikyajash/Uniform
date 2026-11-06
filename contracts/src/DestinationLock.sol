// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {IAqua} from "@1inch/aqua/contracts/interfaces/IAqua.sol";

/// @title DestinationLock
/// @notice The destination lock of a Uniform cross-chain payment, on the chain where the shop is
/// paid. The PayoutPool opens a lock holding a provider's USDT for a merchant, under the same
/// hashlock as the customer's source lock and an earlier expiry:
///   - claim(lockId, secret) before expiry pays the merchant (destination redeem) and emits
///     Settled with the secret, which the provider then uses to redeem the source lock;
///   - refund(lockId) at or after expiry returns the USDT to the provider. It is pushed back into
///     the provider's Aqua strategy, restoring the allowance; if the strategy has been docked, the
///     push reverts and the USDT is transferred to the provider's wallet instead.
/// Each source lock can be filled once: lockId = keccak256(abi.encode(LOCK_TAG, chainid, this,
/// srcChainId, srcLockId)).
contract DestinationLock {
    using SafeERC20 for IERC20;

    enum State {
        None,
        Open,
        Claimed,
        Refunded
    }

    struct OpenParams {
        bytes32 orderId;
        address merchant;
        uint256 amount;
        bytes32 hashlock;
        uint64 expiry;
        uint256 srcChainId;
        bytes32 srcLockId;
        address maker;
        bytes32 strategyHash;
    }

    struct Lock {
        address maker;
        uint64 expiry;
        State state;
        address merchant;
        uint96 amount;
        bytes32 hashlock;
        bytes32 orderId;
        bytes32 strategyHash;
        bytes32 srcLockId;
        uint64 srcChainId;
    }

    bytes32 public constant LOCK_TAG = keccak256("uniform.destinationlock.lock.v1");
    uint256 public constant MIN_LOCK_TIME = 5 minutes;
    uint256 public constant MAX_LOCK_TIME = 2 hours;

    IAqua public immutable aqua;
    IERC20 public immutable usdt;
    address public immutable pool;

    mapping(bytes32 lockId => Lock) public locks;

    event Opened(
        bytes32 indexed lockId,
        bytes32 indexed orderId,
        address indexed merchant,
        address maker,
        uint256 amount,
        bytes32 hashlock,
        uint64 expiry,
        uint256 srcChainId,
        bytes32 srcLockId
    );
    event Settled(
        bytes32 indexed lockId,
        bytes32 indexed orderId,
        address indexed merchant,
        uint256 amount,
        uint256 srcChainId,
        bytes32 srcLockId,
        bytes32 secret
    );
    event Refunded(bytes32 indexed lockId, address indexed maker, uint256 amount, bool backToAqua);

    error NotPool(address caller);
    error ZeroAmount();
    error AmountTooLarge(uint256 amount);
    error InvalidMerchant(address merchant);
    error ZeroHashlock();
    error ExpiryOutOfRange(uint64 expiry);
    error ChainIdTooLarge(uint256 srcChainId);
    error LockExists(bytes32 lockId);
    error NotOpen(bytes32 lockId);
    error LockExpired(bytes32 lockId);
    error NotExpired(bytes32 lockId);
    error WrongSecret(bytes32 lockId);

    /// @dev Deployed by the PayoutPool, which becomes the only address allowed to open locks.
    constructor(IAqua aqua_, IERC20 usdt_) {
        aqua = aqua_;
        usdt = usdt_;
        pool = msg.sender;
    }

    function lockIdFor(uint256 srcChainId, bytes32 srcLockId) public view returns (bytes32) {
        return keccak256(abi.encode(LOCK_TAG, block.chainid, address(this), srcChainId, srcLockId));
    }

    /// @notice Records a lock. The pool moves the USDT in right after, in the same transaction.
    function open(OpenParams calldata p) external returns (bytes32 lockId) {
        if (msg.sender != pool) revert NotPool(msg.sender);
        if (p.amount == 0) revert ZeroAmount();
        if (p.amount > type(uint96).max) revert AmountTooLarge(p.amount);
        if (
            p.merchant == address(0) || p.merchant == address(this) || p.merchant == pool || p.merchant == address(usdt)
        ) {
            revert InvalidMerchant(p.merchant);
        }
        if (p.hashlock == bytes32(0)) revert ZeroHashlock();
        if (p.expiry < block.timestamp + MIN_LOCK_TIME || p.expiry > block.timestamp + MAX_LOCK_TIME) {
            revert ExpiryOutOfRange(p.expiry);
        }
        if (p.srcChainId > type(uint64).max) revert ChainIdTooLarge(p.srcChainId);

        lockId = lockIdFor(p.srcChainId, p.srcLockId);
        if (locks[lockId].state != State.None) revert LockExists(lockId);

        locks[lockId] = Lock({
            maker: p.maker,
            expiry: p.expiry,
            state: State.Open,
            merchant: p.merchant,
            amount: uint96(p.amount),
            hashlock: p.hashlock,
            orderId: p.orderId,
            strategyHash: p.strategyHash,
            srcLockId: p.srcLockId,
            srcChainId: uint64(p.srcChainId)
        });
        emit Opened(lockId, p.orderId, p.merchant, p.maker, p.amount, p.hashlock, p.expiry, p.srcChainId, p.srcLockId);
    }

    /// @notice Destination redeem: pays the merchant if `secret` matches the hashlock before expiry.
    function claim(bytes32 lockId, bytes32 secret) external {
        Lock storage l = locks[lockId];
        if (l.state != State.Open) revert NotOpen(lockId);
        if (block.timestamp >= l.expiry) revert LockExpired(lockId);
        if (keccak256(abi.encodePacked(secret)) != l.hashlock) revert WrongSecret(lockId);
        l.state = State.Claimed;
        usdt.safeTransfer(l.merchant, l.amount);
        emit Settled(lockId, l.orderId, l.merchant, l.amount, l.srcChainId, l.srcLockId, secret);
    }

    /// @notice Returns the USDT to the provider once the lock has expired. Anyone may call it.
    function refund(bytes32 lockId) external {
        Lock storage l = locks[lockId];
        if (l.state != State.Open) revert NotOpen(lockId);
        if (block.timestamp < l.expiry) revert NotExpired(lockId);
        l.state = State.Refunded;

        address maker = l.maker;
        uint256 amount = l.amount;
        bool backToAqua;
        usdt.forceApprove(address(aqua), amount);
        try aqua.push(maker, pool, l.strategyHash, address(usdt), amount) {
            backToAqua = true;
        } catch {
            usdt.forceApprove(address(aqua), 0);
            usdt.safeTransfer(maker, amount);
        }
        emit Refunded(lockId, maker, amount, backToAqua);
    }
}
