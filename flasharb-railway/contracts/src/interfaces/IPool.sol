// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/**
 * @title IPool
 * @notice Minimal Aave V3 Pool interface exposing the single-asset flash loan
 *         entrypoint and the premium getter used for profitability checks.
 */
interface IPool {
    /**
     * @notice Allows smart contracts to access the liquidity of the pool within
     *         one transaction, as long as the amount taken plus a fee is returned.
     * @param receiverAddress The address of the contract receiving the funds,
     *        implementing IFlashLoanSimpleReceiver.
     * @param asset The address of the asset being flash-borrowed.
     * @param amount The amount of the asset being flash-borrowed.
     * @param params Arbitrary bytes-encoded params passed to executeOperation.
     * @param referralCode Referral code (0 if the action is executed directly).
     */
    function flashLoanSimple(
        address receiverAddress,
        address asset,
        uint256 amount,
        bytes calldata params,
        uint16 referralCode
    ) external;

    /**
     * @notice Returns the total flash loan premium, expressed in bps (e.g. 5 = 0.05%).
     */
    function FLASHLOAN_PREMIUM_TOTAL() external view returns (uint128);
}
