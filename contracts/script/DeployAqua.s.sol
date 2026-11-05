// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Script, console } from "forge-std/Script.sol";
import { VmSafe } from "forge-std/Vm.sol";
import { AquaRouter } from "@1inch/aqua/contracts/AquaRouter.sol";

/// Deploys 1inch's unmodified AquaRouter (Aqua core + Multicall + Simulator + Rescuable)
/// on Whitechain Sepolia. The broadcasting account becomes the rescue owner.
contract DeployAqua is Script {
    function run() external returns (AquaRouter aqua) {
        require(block.chainid == 1874, "not Whitechain Sepolia");
        vm.startBroadcast();
        (, address broadcaster,) = vm.readCallers();
        aqua = new AquaRouter(broadcaster);
        vm.stopBroadcast();
        console.log("AquaRouter:", address(aqua));
    }
}
