// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Script, console} from "forge-std/Script.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {IAqua} from "@1inch/aqua/contracts/interfaces/IAqua.sol";
import {PayoutPool} from "../src/PayoutPool.sol";

/// Deploys PayoutPool on Whitechain Sepolia for our Aqua and USDT. The pool deploys its own
/// DestinationLock in its constructor.
contract DeployPayoutPool is Script {
    IAqua constant AQUA = IAqua(0x02dF2659B5B4956EAFEe3293584C2aaefC114871);
    IERC20 constant USDT = IERC20(0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7);

    function run() external returns (PayoutPool pool) {
        require(block.chainid == 1874, "not Whitechain Sepolia");
        vm.startBroadcast();
        pool = new PayoutPool(AQUA, USDT);
        vm.stopBroadcast();
        console.log("PayoutPool:", address(pool));
        console.log("DestinationLock:", address(pool.destinationLock()));
    }
}
