// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import { IFlashLoanSimpleReceiver } from "./interfaces/IFlashLoanSimpleReceiver.sol";
import { IPoolAddressesProvider } from "./interfaces/IPoolAddressesProvider.sol";
import { IPool } from "./interfaces/IPool.sol";
import { IERC20 } from "./interfaces/IERC20.sol";
import { IUniswapV2Router } from "./interfaces/IUniswapV2Router.sol";
import { IUniswapV3Router } from "./interfaces/IUniswapV3Router.sol";

/**
 * @title FlashArbExecutor
 * @author Loxee
 * @notice On-chain executor for cross-DEX arbitrage funded by Aave V3 flash loans.
 *
 * Flow per opportunity:
 *   1. Off-chain bot detects a price spread for a token pair between DEX A and DEX B.
 *   2. Bot calls {executeArbitrage}, which requests a single-asset flash loan from Aave.
 *   3. Aave sends `amount` of `loanAsset` and invokes {executeOperation}.
 *   4. We buy the intermediate token cheap on the "buy" DEX, then sell it back into
 *      `loanAsset` on the "sell" DEX, capturing the spread.
 *   5. We must end the call holding at least `amount + premium` of `loanAsset` so Aave
 *      can pull repayment. Anything above that is profit, swept to the owner.
 *
 * @dev Supports Uniswap V2-style and Uniswap V3-style routers on either leg via the
 *      {Dex} descriptor. The contract is intentionally permissioned: only the owner
 *      may initiate loans, since `executeOperation` will execute arbitrary swaps.
 */
contract FlashArbExecutor is IFlashLoanSimpleReceiver {
    /*//////////////////////////////////////////////////////////////
                                 TYPES
    //////////////////////////////////////////////////////////////*/

    enum DexKind {
        UniswapV2,
        UniswapV3
    }

    /// @notice Describes one leg of the arbitrage route.
    struct Dex {
        DexKind kind; // V2 or V3 router semantics
        address router; // router contract address on the target chain
        uint24 fee; // V3 fee tier (e.g. 500/3000/10000); ignored for V2
    }

    /// @notice Decoded payload carried through the flash loan via `params`.
    struct ArbParams {
        address midToken; // intermediate token bought on the first leg
        Dex buyDex; // where we spend loanAsset to acquire midToken (cheap side)
        Dex sellDex; // where we sell midToken back to loanAsset (expensive side)
        uint256 minProfit; // minimum profit in loanAsset units; reverts if not met
    }

    /*//////////////////////////////////////////////////////////////
                                STORAGE
    //////////////////////////////////////////////////////////////*/

    IPoolAddressesProvider public immutable ADDRESSES_PROVIDER;
    IPool public immutable POOL;
    address public owner;

    /*//////////////////////////////////////////////////////////////
                                EVENTS
    //////////////////////////////////////////////////////////////*/

    event ArbitrageExecuted(
        address indexed loanAsset,
        address indexed midToken,
        uint256 loanAmount,
        uint256 premium,
        uint256 profit
    );
    event ProfitWithdrawn(address indexed token, address indexed to, uint256 amount);
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);

    /*//////////////////////////////////////////////////////////////
                                ERRORS
    //////////////////////////////////////////////////////////////*/

    error NotOwner();
    error NotPool();
    error BadInitiator();
    error UnprofitableTrade(uint256 received, uint256 owed, uint256 minProfit);

    /*//////////////////////////////////////////////////////////////
                              MODIFIERS
    //////////////////////////////////////////////////////////////*/

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    /*//////////////////////////////////////////////////////////////
                             CONSTRUCTOR
    //////////////////////////////////////////////////////////////*/

    /**
     * @param addressesProvider Aave V3 PoolAddressesProvider for the target chain.
     */
    constructor(address addressesProvider) {
        ADDRESSES_PROVIDER = IPoolAddressesProvider(addressesProvider);
        POOL = IPool(IPoolAddressesProvider(addressesProvider).getPool());
        owner = msg.sender;
        emit OwnershipTransferred(address(0), msg.sender);
    }

    /*//////////////////////////////////////////////////////////////
                          EXTERNAL ENTRYPOINT
    //////////////////////////////////////////////////////////////*/

    /**
     * @notice Initiates a flash-loan-funded arbitrage.
     * @param loanAsset The asset to borrow and repay (also the profit denomination).
     * @param amount The amount of `loanAsset` to flash-borrow.
     * @param params The {ArbParams} describing the route and guardrails.
     */
    function executeArbitrage(address loanAsset, uint256 amount, ArbParams calldata params) external onlyOwner {
        POOL.flashLoanSimple(address(this), loanAsset, amount, abi.encode(params), 0);
    }

    /*//////////////////////////////////////////////////////////////
                        AAVE FLASH LOAN CALLBACK
    //////////////////////////////////////////////////////////////*/

    /**
     * @inheritdoc IFlashLoanSimpleReceiver
     * @dev Called by the Aave Pool mid-transaction. Executes both swap legs and
     *      verifies profitability before approving repayment.
     */
    function executeOperation(
        address asset,
        uint256 amount,
        uint256 premium,
        address initiator,
        bytes calldata params
    ) external override returns (bool) {
        if (msg.sender != address(POOL)) revert NotPool();
        if (initiator != address(this)) revert BadInitiator();

        ArbParams memory p = abi.decode(params, (ArbParams));

        // Leg 1: spend the borrowed `asset` to buy `midToken` on the cheap DEX.
        uint256 midReceived = _swap(p.buyDex, asset, p.midToken, amount, 0);

        // Leg 2: sell the full `midToken` balance back to `asset` on the rich DEX.
        uint256 assetReceived = _swap(p.sellDex, p.midToken, asset, midReceived, 0);

        uint256 amountOwed = amount + premium;
        if (assetReceived < amountOwed + p.minProfit) {
            revert UnprofitableTrade(assetReceived, amountOwed, p.minProfit);
        }

        // Approve the Pool to pull principal + premium during repayment.
        _approveIfNeeded(asset, address(POOL), amountOwed);

        uint256 profit = assetReceived - amountOwed;
        emit ArbitrageExecuted(asset, p.midToken, amount, premium, profit);

        return true;
    }

    /*//////////////////////////////////////////////////////////////
                            INTERNAL SWAPS
    //////////////////////////////////////////////////////////////*/

    /**
     * @dev Routes a single-hop exact-input swap through the appropriate router type.
     */
    function _swap(Dex memory dex, address tokenIn, address tokenOut, uint256 amountIn, uint256 minOut)
        internal
        returns (uint256 amountOut)
    {
        _approveIfNeeded(tokenIn, dex.router, amountIn);

        if (dex.kind == DexKind.UniswapV2) {
            address[] memory path = new address[](2);
            path[0] = tokenIn;
            path[1] = tokenOut;
            uint256[] memory amounts = IUniswapV2Router(dex.router).swapExactTokensForTokens(
                amountIn, minOut, path, address(this), block.timestamp
            );
            amountOut = amounts[amounts.length - 1];
        } else {
            IUniswapV3Router.ExactInputSingleParams memory swapParams = IUniswapV3Router.ExactInputSingleParams({
                tokenIn: tokenIn,
                tokenOut: tokenOut,
                fee: dex.fee,
                recipient: address(this),
                deadline: block.timestamp,
                amountIn: amountIn,
                amountOutMinimum: minOut,
                sqrtPriceLimitX96: 0
            });
            amountOut = IUniswapV3Router(dex.router).exactInputSingle(swapParams);
        }
    }

    /**
     * @dev Sets a max allowance lazily to save gas on repeated routes.
     */
    function _approveIfNeeded(address token, address spender, uint256 amount) internal {
        if (IERC20(token).allowance(address(this), spender) < amount) {
            IERC20(token).approve(spender, type(uint256).max);
        }
    }

    /*//////////////////////////////////////////////////////////////
                            OWNER ACTIONS
    //////////////////////////////////////////////////////////////*/

    /**
     * @notice Sweeps accumulated profit (or any stranded tokens) to the owner.
     */
    function withdraw(address token) external onlyOwner {
        uint256 bal = IERC20(token).balanceOf(address(this));
        IERC20(token).transfer(owner, bal);
        emit ProfitWithdrawn(token, owner, bal);
    }

    /**
     * @notice Transfers contract ownership.
     */
    function transferOwnership(address newOwner) external onlyOwner {
        emit OwnershipTransferred(owner, newOwner);
        owner = newOwner;
    }
}
