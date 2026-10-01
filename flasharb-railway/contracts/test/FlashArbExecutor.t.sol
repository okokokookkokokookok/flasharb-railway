// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import { Test } from "forge-std/Test.sol";
import { FlashArbExecutor } from "../src/FlashArbExecutor.sol";
import { IERC20 } from "../src/interfaces/IERC20.sol";
import { IUniswapV2Router } from "../src/interfaces/IUniswapV2Router.sol";

/*//////////////////////////////////////////////////////////////
                              MOCKS
//////////////////////////////////////////////////////////////*/

/// @notice Minimal mintable ERC20 used to back the simulated DEX liquidity.
contract MockERC20 {
    string public name;
    uint8 public decimals = 18;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    constructor(string memory _name) {
        name = _name;
    }

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        _transfer(msg.sender, to, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        uint256 allowed = allowance[from][msg.sender];
        if (allowed != type(uint256).max) {
            allowance[from][msg.sender] = allowed - amount;
        }
        _transfer(from, to, amount);
        return true;
    }

    function _transfer(address from, address to, uint256 amount) internal {
        require(balanceOf[from] >= amount, "INSUFFICIENT_BALANCE");
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
    }
}

/// @notice DEX mock that swaps at a fixed price ratio (priceBps / 10_000).
/// @dev Lets us simulate a spread: a "cheap" DEX gives more midToken per loanAsset,
///      and a "rich" DEX gives more loanAsset per midToken.
contract MockV2Router is IUniswapV2Router {
    // rate from path[0] -> path[1], scaled by 1e18
    mapping(address => mapping(address => uint256)) public rate1e18;

    function setRate(address tokenIn, address tokenOut, uint256 rate) external {
        rate1e18[tokenIn][tokenOut] = rate;
    }

    function getAmountsOut(uint256 amountIn, address[] calldata path)
        external
        view
        override
        returns (uint256[] memory amounts)
    {
        amounts = new uint256[](2);
        amounts[0] = amountIn;
        amounts[1] = (amountIn * rate1e18[path[0]][path[1]]) / 1e18;
    }

    function swapExactTokensForTokens(
        uint256 amountIn,
        uint256 amountOutMin,
        address[] calldata path,
        address to,
        uint256
    ) external override returns (uint256[] memory amounts) {
        uint256 out = (amountIn * rate1e18[path[0]][path[1]]) / 1e18;
        require(out >= amountOutMin, "SLIPPAGE");
        MockERC20(path[0]).transferFrom(msg.sender, address(this), amountIn);
        MockERC20(path[1]).transfer(to, out);
        amounts = new uint256[](2);
        amounts[0] = amountIn;
        amounts[1] = out;
    }
}

/// @notice Aave Pool mock implementing flashLoanSimple with a configurable premium.
contract MockPool {
    uint128 public constant FLASHLOAN_PREMIUM_TOTAL = 5; // 0.05% in bps

    function flashLoanSimple(
        address receiver,
        address asset,
        uint256 amount,
        bytes calldata params,
        uint16
    ) external {
        uint256 premium = (amount * FLASHLOAN_PREMIUM_TOTAL) / 10_000;
        MockERC20(asset).transfer(receiver, amount);

        bool ok = FlashArbExecutor(receiver).executeOperation(asset, amount, premium, receiver, params);
        require(ok, "CALLBACK_FAILED");

        // Pull repayment (principal + premium) — receiver approved us in the callback.
        MockERC20(asset).transferFrom(receiver, address(this), amount + premium);
    }
}

/// @notice AddressesProvider mock pointing at the pool.
contract MockAddressesProvider {
    address public pool;
    address public priceOracle;

    constructor(address _pool) {
        pool = _pool;
    }

    function getPool() external view returns (address) {
        return pool;
    }

    function getPriceOracle() external view returns (address) {
        return priceOracle;
    }
}

/*//////////////////////////////////////////////////////////////
                              TESTS
//////////////////////////////////////////////////////////////*/

contract FlashArbExecutorTest is Test {
    MockERC20 internal usdc; // loan asset
    MockERC20 internal weth; // intermediate token
    MockV2Router internal cheapDex; // buy WETH cheap here
    MockV2Router internal richDex; // sell WETH high here
    MockPool internal pool;
    MockAddressesProvider internal provider;
    FlashArbExecutor internal executor;

    address internal owner = address(0xA11CE);

    function setUp() public {
        usdc = new MockERC20("USDC");
        weth = new MockERC20("WETH");
        cheapDex = new MockV2Router();
        richDex = new MockV2Router();
        pool = new MockPool();
        provider = new MockAddressesProvider(address(pool));

        vm.prank(owner);
        executor = new FlashArbExecutor(address(provider));

        // Seed the pool with USDC liquidity for the loan.
        usdc.mint(address(pool), 1_000_000e18);

        // Configure the spread:
        //   cheapDex: 1 USDC -> 0.0006 WETH  (i.e. WETH ~ 1666 USDC)  [we buy here]
        //   richDex : 1 WETH -> 1750 USDC                              [we sell here]
        cheapDex.setRate(address(usdc), address(weth), 0.0006e18);
        richDex.setRate(address(weth), address(usdc), 1750e18);

        // Seed DEX inventories so swaps can pay out.
        weth.mint(address(cheapDex), 1_000e18);
        usdc.mint(address(richDex), 5_000_000e18);
    }

    function _buildParams(uint256 minProfit) internal view returns (FlashArbExecutor.ArbParams memory) {
        return FlashArbExecutor.ArbParams({
            midToken: address(weth),
            buyDex: FlashArbExecutor.Dex({
                kind: FlashArbExecutor.DexKind.UniswapV2,
                router: address(cheapDex),
                fee: 0
            }),
            sellDex: FlashArbExecutor.Dex({
                kind: FlashArbExecutor.DexKind.UniswapV2,
                router: address(richDex),
                fee: 0
            }),
            minProfit: minProfit
        });
    }

    function test_ProfitableArbitrageRepaysAndAccruesProfit() public {
        uint256 loan = 100_000e18; // borrow 100k USDC

        // Expected: 100k USDC -> 60 WETH -> 105k USDC. Owe 100.05k. Profit ~4.95k.
        vm.prank(owner);
        executor.executeArbitrage(address(usdc), loan, _buildParams(1_000e18));

        uint256 profit = usdc.balanceOf(address(executor));
        assertGt(profit, 0, "no profit captured");

        // 60 WETH * 1750 = 105_000 USDC out; owe 100_050 USDC; profit = 4_950 USDC.
        assertEq(profit, 4_950e18, "unexpected profit amount");
    }

    function test_WithdrawSweepsProfitToOwner() public {
        vm.prank(owner);
        executor.executeArbitrage(address(usdc), 100_000e18, _buildParams(1_000e18));

        uint256 profit = usdc.balanceOf(address(executor));
        vm.prank(owner);
        executor.withdraw(address(usdc));

        assertEq(usdc.balanceOf(owner), profit, "owner did not receive profit");
        assertEq(usdc.balanceOf(address(executor)), 0, "executor not swept");
    }

    function test_RevertsWhenBelowMinProfit() public {
        // Demand an absurd min profit so the guardrail trips.
        vm.prank(owner);
        vm.expectRevert();
        executor.executeArbitrage(address(usdc), 100_000e18, _buildParams(1_000_000e18));
    }

    function test_RevertsWhenSpreadIsNegative() public {
        // Flip the rich DEX to price WETH below the cheap DEX -> loss-making.
        richDex.setRate(address(weth), address(usdc), 1_500e18);
        vm.prank(owner);
        vm.expectRevert();
        executor.executeArbitrage(address(usdc), 100_000e18, _buildParams(0));
    }

    function test_OnlyOwnerCanInitiate() public {
        vm.prank(address(0xBEEF));
        vm.expectRevert(FlashArbExecutor.NotOwner.selector);
        executor.executeArbitrage(address(usdc), 100_000e18, _buildParams(0));
    }

    function test_ExecuteOperationRejectsNonPoolCaller() public {
        vm.prank(address(0xBEEF));
        vm.expectRevert(FlashArbExecutor.NotPool.selector);
        executor.executeOperation(address(usdc), 1e18, 0, address(executor), "");
    }
}
