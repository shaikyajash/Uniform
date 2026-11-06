// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SourceLock} from "../src/SourceLock.sol";
import {Sign3009} from "./utils/Sign3009.sol";

/// Runs SourceLock against the real Base Sepolia USDC on a local fork. Nothing is sent.
/// Run with: forge test --match-contract SourceLockForkTest
contract SourceLockForkTest is Sign3009 {
    IERC20 constant USDC = IERC20(0x036CbD53842c5426634e7929541eC2318f3dCF7e);

    SourceLock lock;
    uint256 customerKey = 0xA11CE;
    address customer;
    address bob = makeAddr("bob");

    function setUp() public {
        vm.createSelectFork("base_sepolia");
        customer = vm.addr(customerKey);
        lock = new SourceLock(USDC);
        deal(address(USDC), customer, 50e6);
    }

    function test_domainSeparatorMatchesWalletDomain() public view {
        bytes32 expected = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("USDC"),
                keccak256("2"),
                uint256(84532),
                address(USDC)
            )
        );
        (bool ok, bytes memory data) = address(USDC).staticcall(abi.encodeWithSignature("DOMAIN_SEPARATOR()"));
        assertTrue(ok);
        assertEq(abi.decode(data, (bytes32)), expected);
    }

    function test_lockAndClaimWithRealUsdc() public {
        bytes32 secret = keccak256("fork-secret");
        SourceLock.LockParams memory p = SourceLock.LockParams({
            orderId: keccak256("order-3001"),
            receiver: bob,
            minUsdcOut: 12e6,
            hashlock: keccak256(abi.encodePacked(secret)),
            expiry: uint64(block.timestamp + 20 minutes),
            outToken: address(0)
        });
        bytes32 nonce = lock.lockIdFor(p, customer, address(USDC), 12e6);
        Auth memory a = signReceive(
            address(USDC), customerKey, address(lock), 12e6, block.timestamp - 1, block.timestamp + 300, nonce
        );
        bytes32 id = lock.lockWithAuthorization(
            p, customer, address(USDC), 12e6, a.validAfter, a.validBefore, a.v, a.r, a.s, ""
        );
        assertEq(USDC.balanceOf(address(lock)), 12e6);

        lock.claim(id, secret);
        assertEq(USDC.balanceOf(bob), 12e6);
    }
}
