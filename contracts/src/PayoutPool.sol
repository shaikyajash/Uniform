// SPDX-License-Identifier: LicenseRef-Degensoft-Aqua-Source-1.1
pragma solidity 0.8.30;

/// @custom:license-url https://github.com/1inch/aqua/blob/main/LICENSES/Aqua-Source-1.1.txt
/// Built on AquaApp, © 2025 Degensoft Ltd.

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {AquaApp} from "@1inch/aqua/contracts/AquaApp.sol";
import {IAqua} from "@1inch/aqua/contracts/interfaces/IAqua.sol";
import {DestinationLock} from "./DestinationLock.sol";

/// @title PayoutPool
/// @notice An Aqua app that pays shops from a liquidity provider's USDT. The provider (maker)
/// ships a Strategy to this app on Aqua with `ship(pool, abi.encode(strategy), [USDT], [allowance])`;
/// the USDT stays in the provider's wallet. The provider's chosen filler calls `fill` for a
/// customer's source lock: a DestinationLock is opened for the merchant and the USDT is pulled
/// from the provider's wallet straight into it. The strategy hash binds the filler and the
/// per-order cap, so nobody else can spend the provider's allowance and no fill can exceed it.
/// `feeBps` is the provider's advertised fee, read off-chain when quoting.
contract PayoutPool is AquaApp {
    struct Strategy {
        address maker;
        address token;
        address filler;
        uint128 maxPerOrder;
        uint16 feeBps;
        bytes32 salt;
    }

    struct Fill {
        bytes32 orderId;
        address merchant;
        uint256 amount;
        bytes32 hashlock;
        uint64 expiry;
        uint256 srcChainId;
        bytes32 srcLockId;
    }

    IERC20 public immutable usdt;
    DestinationLock public immutable destinationLock;

    event Filled(
        bytes32 indexed orderId, address indexed maker, bytes32 indexed strategyHash, uint256 amount, bytes32 lockId
    );

    error NotFiller(address caller);
    error WrongToken(address token);
    error AboveMaxPerOrder(uint256 amount, uint256 maxPerOrder);

    constructor(IAqua aqua_, IERC20 usdt_) AquaApp(aqua_) {
        usdt = usdt_;
        destinationLock = new DestinationLock(aqua_, usdt_);
    }

    /// @notice The Aqua strategy hash for a strategy shipped as `abi.encode(s)`.
    function strategyHashOf(Strategy calldata s) public pure returns (bytes32) {
        return keccak256(abi.encode(s));
    }

    /// @notice Opens a destination lock for the merchant, funded from the maker's Aqua allowance.
    function fill(Strategy calldata s, Fill calldata f) external returns (bytes32 lockId) {
        if (msg.sender != s.filler) revert NotFiller(msg.sender);
        if (s.token != address(usdt)) revert WrongToken(s.token);
        if (f.amount > s.maxPerOrder) revert AboveMaxPerOrder(f.amount, s.maxPerOrder);

        bytes32 strategyHash = strategyHashOf(s);
        lockId = destinationLock.open(
            DestinationLock.OpenParams({
                orderId: f.orderId,
                merchant: f.merchant,
                amount: f.amount,
                hashlock: f.hashlock,
                expiry: f.expiry,
                srcChainId: f.srcChainId,
                srcLockId: f.srcLockId,
                maker: s.maker,
                strategyHash: strategyHash
            })
        );
        AQUA.pull(s.maker, strategyHash, address(usdt), f.amount, address(destinationLock));
        emit Filled(f.orderId, s.maker, strategyHash, f.amount, lockId);
    }
}
