// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Test } from "forge-std/Test.sol";

interface IDomain {
    function DOMAIN_SEPARATOR() external view returns (bytes32);
}

/// Signs EIP-3009 ReceiveWithAuthorization messages the way a wallet's eth_signTypedData_v4 would.
abstract contract Sign3009 is Test {
    bytes32 internal constant RECEIVE_TYPEHASH = keccak256(
        "ReceiveWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)"
    );

    struct Auth {
        uint256 validAfter;
        uint256 validBefore;
        uint8 v;
        bytes32 r;
        bytes32 s;
    }

    function signReceive(
        address token,
        uint256 signerKey,
        address to,
        uint256 value,
        uint256 validAfter,
        uint256 validBefore,
        bytes32 nonce
    ) internal view returns (Auth memory a) {
        address from = vm.addr(signerKey);
        bytes32 structHash = keccak256(abi.encode(RECEIVE_TYPEHASH, from, to, value, validAfter, validBefore, nonce));
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", IDomain(token).DOMAIN_SEPARATOR(), structHash));
        (a.v, a.r, a.s) = vm.sign(signerKey, digest);
        a.validAfter = validAfter;
        a.validBefore = validBefore;
    }
}
