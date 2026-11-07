// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Script, console } from "forge-std/Script.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { DirectPay } from "../src/DirectPay.sol";

/// Deploys DirectPay for Whitechain Sepolia USDT.
contract DeployDirectPay is Script {
    IERC20 constant USDT = IERC20(0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7);

    function run() external returns (DirectPay directPay) {
        require(block.chainid == 1874, "not Whitechain Sepolia");
        vm.startBroadcast();
        directPay = new DirectPay(USDT);
        vm.stopBroadcast();
        console.log("DirectPay:", address(directPay));
    }
}
