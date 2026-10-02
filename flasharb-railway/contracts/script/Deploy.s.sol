// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import { Script, console2 } from "forge-std/Script.sol";
import { FlashArbExecutor } from "../src/FlashArbExecutor.sol";

/**
 * @title DeployFlashArbExecutor
 * @notice Deploys the executor against the Aave V3 PoolAddressesProvider for the
 *         selected chain. Set AAVE_ADDRESSES_PROVIDER in the environment.
 *
 * Known Aave V3 PoolAddressesProvider addresses:
 *   Base     0xe20fCBdBfFC4Dd138cE8b2E6FBb6CB49777ad64D
 *   Arbitrum 0xa97684ead0e402dC232d5A977953DF7ECBaB3CDb
 *   Optimism 0xa97684ead0e402dC232d5A977953DF7ECBaB3CDb
 *   Polygon  0xa97684ead0e402dC232d5A977953DF7ECBaB3CDb
 *
 * Usage:
 *   forge script script/Deploy.s.sol \
 *     --rpc-url base --broadcast --verify
 */
contract DeployFlashArbExecutor is Script {
    function run() external returns (FlashArbExecutor executor) {
        address provider = vm.envAddress("AAVE_ADDRESSES_PROVIDER");
        uint256 pk = vm.envUint("PRIVATE_KEY");

        vm.startBroadcast(pk);
        executor = new FlashArbExecutor(provider);
        vm.stopBroadcast();

        console2.log("FlashArbExecutor deployed at:", address(executor));
        console2.log("Using AddressesProvider:", provider);
    }
}
