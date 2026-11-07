// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Script, console} from "forge-std/Script.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SourceLock} from "../src/SourceLock.sol";

/// Deploys SourceLock for the chain's Circle USDC and EURC (Base Sepolia or Ethereum Sepolia).
contract DeploySourceLock is Script {
    function run() external returns (SourceLock sourceLock) {
        address usdc;
        address eurc;
        if (block.chainid == 84532) {
            usdc = 0x036CbD53842c5426634e7929541eC2318f3dCF7e;
            eurc = 0x808456652fdb597867f38412077A9182bf77359F;
        } else if (block.chainid == 11155111) {
            usdc = 0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238;
            eurc = 0x08210F9170F89Ab7658F0B5E3fF39b0E03C594D4;
        } else {
            revert("unsupported chain");
        }
        vm.startBroadcast();
        sourceLock = new SourceLock(IERC20(usdc), IERC20(eurc));
        vm.stopBroadcast();
        console.log("SourceLock:", address(sourceLock));
    }
}
