// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import { IPoolAddressesProvider } from "./IPoolAddressesProvider.sol";
import { IPool } from "./IPool.sol";

/**
 * @title IFlashLoanSimpleReceiver
 * @notice Interface that a contract must implement to receive a single-asset
 *         Aave V3 flash loan via `Pool.flashLoanSimple`.
 * @dev Mirrors the canonical Aave V3 core interface so the bytecode is
 *      ABI-compatible with the live Aave deployments on Base, Arbitrum,
 *      Optimism and Polygon.
 */
interface IFlashLoanSimpleReceiver {
    /**
     * @notice Executes an operation after receiving the flash-borrowed asset.
     * @dev Ensure the contract can repay `amount + premium` of `asset` by the
     *      end of this call. The Pool will pull the owed amount via `transferFrom`.
     * @param asset The address of the flash-borrowed asset.
     * @param amount The amount of the flash-borrowed asset.
     * @param premium The fee owed on the flash-borrowed asset.
     * @param initiator The address that initiated the flash loan.
     * @param params Arbitrary encoded params passed through from the initiator.
     * @return True if the execution of the operation succeeds, false otherwise.
     */
    function executeOperation(
        address asset,
        uint256 amount,
        uint256 premium,
        address initiator,
        bytes calldata params
    ) external returns (bool);

    function ADDRESSES_PROVIDER() external view returns (IPoolAddressesProvider);

    function POOL() external view returns (IPool);
}
