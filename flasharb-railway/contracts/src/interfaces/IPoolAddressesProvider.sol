// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/**
 * @title IPoolAddressesProvider
 * @notice Minimal Aave V3 addresses-provider interface used to resolve the Pool.
 */
interface IPoolAddressesProvider {
    function getPool() external view returns (address);

    function getPriceOracle() external view returns (address);
}
