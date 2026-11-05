// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Test } from "forge-std/Test.sol";
import { AquaRouter } from "@1inch/aqua/contracts/AquaRouter.sol";
import { ERC20 } from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

contract TestUSDT is ERC20 {
    constructor() ERC20("Tether USD", "USDT") { }
    function decimals() public pure override returns (uint8) { return 6; }
    function mint(address to, uint256 amount) external { _mint(to, amount); }
}

/// Checks the Aqua behaviour our Payout Pool relies on: ship an allowance,
/// pull from the maker's wallet to any address, push back to restore it, dock.
contract AquaTest is Test {
    AquaRouter aqua;
    TestUSDT usdt;
    address bob = makeAddr("bob");
    address app = makeAddr("payoutPool");
    address shopLock = makeAddr("shopLock");
    bytes strategy = abi.encode(bob, "uniform-strategy");
    bytes32 strategyHash;

    function setUp() public {
        aqua = new AquaRouter(address(this));
        usdt = new TestUSDT();
        usdt.mint(bob, 100e6);
        vm.startPrank(bob);
        usdt.approve(address(aqua), type(uint256).max);
        address[] memory tokens = new address[](1);
        tokens[0] = address(usdt);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 50e6;
        strategyHash = aqua.ship(app, strategy, tokens, amounts);
        vm.stopPrank();
    }

    function test_shipKeepsTokensInWallet() public view {
        assertEq(usdt.balanceOf(bob), 100e6);
        assertEq(usdt.balanceOf(address(aqua)), 0);
        (uint248 balance,) = aqua.rawBalances(bob, app, strategyHash, address(usdt));
        assertEq(balance, 50e6);
    }

    function test_appPullsFromMakerWalletToShopLock() public {
        vm.prank(app);
        aqua.pull(bob, strategyHash, address(usdt), 12e6, shopLock);
        assertEq(usdt.balanceOf(shopLock), 12e6);
        assertEq(usdt.balanceOf(bob), 88e6);
        (uint248 balance,) = aqua.rawBalances(bob, app, strategyHash, address(usdt));
        assertEq(balance, 38e6);
    }

    function test_pullBeyondAllowanceReverts() public {
        vm.prank(app);
        vm.expectRevert();
        aqua.pull(bob, strategyHash, address(usdt), 51e6, shopLock);
    }

    function test_onlyTheShippedAppCanPull() public {
        vm.prank(makeAddr("stranger"));
        vm.expectRevert();
        aqua.pull(bob, strategyHash, address(usdt), 1e6, shopLock);
    }

    function test_pushRestoresAllowanceAndPaysMaker() public {
        vm.prank(app);
        aqua.pull(bob, strategyHash, address(usdt), 12e6, shopLock);
        vm.startPrank(shopLock);
        usdt.approve(address(aqua), 12e6);
        aqua.push(bob, app, strategyHash, address(usdt), 12e6);
        vm.stopPrank();
        assertEq(usdt.balanceOf(bob), 100e6);
        (uint248 balance,) = aqua.rawBalances(bob, app, strategyHash, address(usdt));
        assertEq(balance, 50e6);
    }

    function test_pushAfterDockReverts() public {
        address[] memory tokens = new address[](1);
        tokens[0] = address(usdt);
        vm.prank(bob);
        aqua.dock(app, strategyHash, tokens);
        usdt.mint(shopLock, 1e6);
        vm.startPrank(shopLock);
        usdt.approve(address(aqua), 1e6);
        vm.expectRevert();
        aqua.push(bob, app, strategyHash, address(usdt), 1e6);
        vm.stopPrank();
    }
}
