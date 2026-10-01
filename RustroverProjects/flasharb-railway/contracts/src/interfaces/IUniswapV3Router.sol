// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/**
 * @title IUniswapV3Router
 * @notice Subset of the Uniswap V3 SwapRouter (ISwapRouter) used for single-hop
 *         exact-input swaps across a specific fee tier.
 */
interface IUniswapV3Router {
    struct ExactInputSingleParams {
        address tokenIn;
        address tokenOut;
        uint24 fee;
        address recipient;
        uint256 deadline;
        uint256 amountIn;
        uint256 amountOutMinimum;
        uint160 sqrtPriceLimitX96;
    }

    /**
     * @notice Swaps `amountIn` of one token for as much as possible of another,
     *         routed through a single V3 pool of the given fee tier.
     * @return amountOut The amount of the received token.
     */
    function exactInputSingle(ExactInputSingleParams calldata params)
        external
        payable
        returns (uint256 amountOut);
}

/**
 * @title IQuoterV2
 * @notice Uniswap V3 QuoterV2 used off-path for read-only price quoting.
 * @dev Marked non-view by Uniswap because it relies on a revert-and-decode
 *      pattern internally; call via staticcall / eth_call only.
 */
interface IQuoterV2 {
    struct QuoteExactInputSingleParams {
        address tokenIn;
        address tokenOut;
        uint256 amountIn;
        uint24 fee;
        uint160 sqrtPriceLimitX96;
    }

    function quoteExactInputSingle(QuoteExactInputSingleParams memory params)
        external
        returns (
            uint256 amountOut,
            uint160 sqrtPriceX96After,
            uint32 initializedTicksCrossed,
            uint256 gasEstimate
        );
}
