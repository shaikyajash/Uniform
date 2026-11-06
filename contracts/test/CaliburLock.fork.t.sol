// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Test, Vm} from "forge-std/Test.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SourceLock} from "../src/SourceLock.sol";

/// Calibur v1.1.0 types and entry points used by the customer batch (see Uniswap's Calibur repo).
struct Call {
    address to;
    uint256 value;
    bytes data;
}

struct BatchedCall {
    Call[] calls;
    bool revertOnFailure;
}

struct SignedBatchedCall {
    BatchedCall batchedCall;
    uint256 nonce;
    bytes32 keyHash;
    address executor;
    uint256 deadline;
}

interface ICalibur {
    function execute(SignedBatchedCall calldata signedBatchedCall, bytes calldata wrappedSignature) external payable;
    function hashTypedData(bytes32 hash) external view returns (bytes32);
    function domainSeparator() external view returns (bytes32);
}

/// A customer's EOA with WBTC and no ETH delegates to the real Calibur (EIP-7702) on an Ethereum
/// Sepolia fork, signs one batch (approve, lock), and a gas payer submits it. Nothing is sent.
/// Run with: forge test --match-contract CaliburLockForkTest (SEPOLIA_RPC_URL overrides the
/// `sepolia` endpoint in foundry.toml).
contract CaliburLockForkTest is Test {
    address constant CALIBUR = 0x000000005c84F8Fd50b21CAC312528A64437030e;
    IERC20 constant WBTC = IERC20(0x29f2D40B0605204364af54EC677bD022dA425d03);
    IERC20 constant USDC = IERC20(0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238);
    IERC20 constant EURC = IERC20(0x08210F9170F89Ab7658F0B5E3fF39b0E03C594D4);

    bytes32 constant DOMAIN_TYPEHASH =
        keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract,bytes32 salt)");
    bytes32 constant CALL_TYPEHASH = keccak256("Call(address to,uint256 value,bytes data)");
    bytes32 constant BATCHED_CALL_TYPEHASH =
        keccak256("BatchedCall(Call[] calls,bool revertOnFailure)Call(address to,uint256 value,bytes data)");
    bytes32 constant SIGNED_BATCHED_CALL_TYPEHASH = keccak256(
        "SignedBatchedCall(BatchedCall batchedCall,uint256 nonce,bytes32 keyHash,address executor,uint256 deadline)BatchedCall(Call[] calls,bool revertOnFailure)Call(address to,uint256 value,bytes data)"
    );

    SourceLock lock;
    Vm.Wallet customer;
    address gasPayer = makeAddr("gasPayer");
    address bob = makeAddr("bob");
    bytes32 secret = keccak256("calibur-fork-secret");
    uint256 constant AMOUNT = 15_000; // 0.00015 WBTC

    function setUp() public {
        vm.createSelectFork(vm.envOr("SEPOLIA_RPC_URL", string("sepolia")));
        assertEq(block.chainid, 11155111);
        lock = new SourceLock(USDC, EURC);
        customer = vm.createWallet("calibur-customer");
        vm.deal(customer.addr, 0);
        deal(address(WBTC), customer.addr, 1e8);
        vm.deal(gasPayer, 1 ether);
    }

    /// Same as server/common/src/calibur.rs `nonce_for`: key = keccak256(lockId) >> 64, sequence 0.
    function nonceFor(bytes32 lockId) internal pure returns (uint256) {
        return (uint256(keccak256(abi.encode(lockId))) >> 64) << 64;
    }

    function hashBatch(SignedBatchedCall memory s) internal pure returns (bytes32) {
        Call[] memory calls = s.batchedCall.calls;
        bytes32[] memory callHashes = new bytes32[](calls.length);
        for (uint256 i; i < calls.length; i++) {
            callHashes[i] = keccak256(abi.encode(CALL_TYPEHASH, calls[i].to, calls[i].value, keccak256(calls[i].data)));
        }
        bytes32 batchHash = keccak256(
            abi.encode(BATCHED_CALL_TYPEHASH, keccak256(abi.encodePacked(callHashes)), s.batchedCall.revertOnFailure)
        );
        return
            keccak256(abi.encode(SIGNED_BATCHED_CALL_TYPEHASH, batchHash, s.nonce, s.keyHash, s.executor, s.deadline));
    }

    function domainSeparator(address account) internal view returns (bytes32) {
        return keccak256(
            abi.encode(
                DOMAIN_TYPEHASH,
                keccak256("Calibur"),
                keccak256("1.0.0"),
                block.chainid,
                account,
                bytes32(uint256(uint160(CALIBUR)))
            )
        );
    }

    function test_customerLocksWbtcThroughCaliburWithoutEth() public {
        SourceLock.LockParams memory p = SourceLock.LockParams({
            orderId: keccak256("order-wbtc-1"),
            receiver: bob,
            minUsdcOut: 12e6,
            hashlock: keccak256(abi.encodePacked(secret)),
            expiry: uint64(block.timestamp + 20 minutes),
            outToken: address(0)
        });
        bytes32 lockId = lock.lockIdFor(p, customer.addr, address(WBTC), AMOUNT);

        Call[] memory calls = new Call[](2);
        calls[0] = Call(address(WBTC), 0, abi.encodeCall(IERC20.approve, (address(lock), AMOUNT)));
        calls[1] = Call(address(lock), 0, abi.encodeCall(SourceLock.lock, (p, address(WBTC), AMOUNT)));
        SignedBatchedCall memory batch = SignedBatchedCall({
            batchedCall: BatchedCall({calls: calls, revertOnFailure: true}),
            nonce: nonceFor(lockId),
            keyHash: bytes32(0),
            executor: address(0),
            deadline: block.timestamp + 600
        });

        // The customer's wallet signs the 7702 authorization naming Calibur, and the batch.
        Vm.SignedDelegation memory auth = vm.signDelegation(CALIBUR, customer.privateKey);
        bytes32 structHash = hashBatch(batch);
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", domainSeparator(customer.addr), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(customer.privateKey, digest);
        bytes memory signature = abi.encodePacked(r, s, v);

        uint256 wbtcBefore = WBTC.balanceOf(customer.addr);

        // The gas payer sends one type-4 transaction: the authorization plus execute on the customer.
        vm.attachDelegation(auth);
        vm.prank(gasPayer, gasPayer);
        ICalibur(customer.addr).execute(batch, abi.encode(signature, bytes("")));

        assertEq(customer.addr.code, abi.encodePacked(hex"ef0100", CALIBUR), "delegated to Calibur");
        assertEq(ICalibur(customer.addr).domainSeparator(), domainSeparator(customer.addr), "domain");
        assertEq(ICalibur(customer.addr).hashTypedData(structHash), digest, "digest");

        (address payer, uint64 expiry, SourceLock.State state, address receiver, uint96 amount,,, address token) =
            lock.locks(lockId);
        assertEq(uint8(state), uint8(SourceLock.State.Open));
        assertEq(payer, customer.addr);
        assertEq(token, address(WBTC));
        assertEq(amount, AMOUNT);
        assertEq(receiver, bob);
        assertEq(expiry, p.expiry);
        assertEq(customer.addr.balance, 0, "customer spent no ETH");
        assertEq(WBTC.balanceOf(customer.addr), wbtcBefore - AMOUNT);
        assertEq(WBTC.balanceOf(address(lock)), AMOUNT);
        assertEq(WBTC.allowance(customer.addr, address(lock)), 0, "approval used up");

        // The same signed batch cannot run twice (nonce used, and the lock exists).
        vm.prank(gasPayer, gasPayer);
        vm.expectRevert();
        ICalibur(customer.addr).execute(batch, abi.encode(signature, bytes("")));

        lock.claim(lockId, secret);
        assertEq(WBTC.balanceOf(bob), AMOUNT);
        assertEq(WBTC.balanceOf(address(lock)), 0);
    }

    function test_aBatchSignedByAnotherKeyIsRejected() public {
        SourceLock.LockParams memory p = SourceLock.LockParams({
            orderId: keccak256("order-wbtc-2"),
            receiver: bob,
            minUsdcOut: 0,
            hashlock: keccak256(abi.encodePacked(secret)),
            expiry: uint64(block.timestamp + 20 minutes),
            outToken: address(0)
        });
        bytes32 lockId = lock.lockIdFor(p, customer.addr, address(WBTC), AMOUNT);
        Call[] memory calls = new Call[](2);
        calls[0] = Call(address(WBTC), 0, abi.encodeCall(IERC20.approve, (address(lock), AMOUNT)));
        calls[1] = Call(address(lock), 0, abi.encodeCall(SourceLock.lock, (p, address(WBTC), AMOUNT)));
        SignedBatchedCall memory batch = SignedBatchedCall({
            batchedCall: BatchedCall({calls: calls, revertOnFailure: true}),
            nonce: nonceFor(lockId),
            keyHash: bytes32(0),
            executor: address(0),
            deadline: block.timestamp + 600
        });
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", domainSeparator(customer.addr), hashBatch(batch)));
        (, uint256 attackerKey) = makeAddrAndKey("attacker");
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(attackerKey, digest);

        vm.signAndAttachDelegation(CALIBUR, customer.privateKey);
        vm.prank(gasPayer, gasPayer);
        vm.expectRevert();
        ICalibur(customer.addr).execute(batch, abi.encode(abi.encodePacked(r, s, v), bytes("")));
        (,, SourceLock.State state,,,,,) = lock.locks(lockId);
        assertEq(uint8(state), uint8(SourceLock.State.None));
    }
}
