// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/**
 * @title IUniswapV2Router
 * @notice Subset of the Uniswap V2 / SushiSwap router used for exact-input swaps
 *         and on-chain price quoting via getAmountsOut.
 */
interface IUniswapV2Router {
    /**
     * @notice Swaps an exact amount of input tokens for as many output tokens as possible.
     * @param amountIn The amount of input tokens to send.
     * @param amountOutMin The minimum amount of output tokens that must be received.
     * @param path An array of token addresses describing the swap route.
     * @param to Recipient of the output tokens.
     * @param deadline Unix timestamp after which the tx reverts.
     * @return amounts The input amount and all subsequent output amounts.
     */
    function swapExactTokensForTokens(
        uint256 amountIn,
        uint256 amountOutMin,
        address[] calldata path,
        address to,
        uint256 deadline
    ) external returns (uint256[] memory amounts);

    /**
     * @notice Given an input amount and a path, returns the expected output amounts.
     */
    function getAmountsOut(uint256 amountIn, address[] calldata path)
        external
        view
        returns (uint256[] memory amounts);
}
