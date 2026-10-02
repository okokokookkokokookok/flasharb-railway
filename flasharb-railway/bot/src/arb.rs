//! Spread math and profitability simulation.
//!
//! The core question: given a quote `q_buy` (how much intermediate token we get
//! for `trade_size` of the base asset on DEX A) and `q_sell` (how much base
//! asset we get back for that intermediate amount on DEX B), is the round-trip
//! profitable *after* the Aave flash-loan premium and the gas cost?

use crate::scanner::PairQuotes;

/// Result of evaluating a single (chain, pair, DEX-A, DEX-B) candidate.
#[derive(Debug, Clone)]
pub struct ArbResult {
    pub chain: String,
    pub pair: String,
    pub buy_dex: String,
    pub sell_dex: String,
    /// Flash-loan size, in base-token whole units.
    pub trade_size: f64,
    /// Base tokens returned after the round trip, before costs.
    pub gross_return: f64,
    /// Gross spread as a fraction of trade size (e.g. 0.012 = 1.2%).
    pub spread_pct: f64,
    /// Aave premium cost, in base-token units.
    pub premium_cost: f64,
    /// Estimated gas cost, converted into base-token units.
    pub gas_cost: f64,
    /// Net profit after premium + gas, in base-token units.
    pub net_profit: f64,
    pub profitable: bool,
}

/// Inputs for a profitability evaluation.
#[derive(Debug, Clone)]
pub struct EvalParams {
    /// Flash-loan premium in basis points (Aave V3: 5 = 0.05%).
    pub premium_bps: u32,
    /// Gas units the arbitrage transaction is expected to burn.
    pub gas_limit: u64,
    /// Gas price in gwei.
    pub gas_price_gwei: f64,
    /// USD value of one unit of the chain's native token.
    pub native_usd: f64,
    /// USD value of one whole base token (for converting gas USD -> base units).
    pub base_usd: f64,
    /// Minimum net profit (in base-token units) to consider it worth firing.
    pub min_profit: f64,
}

/// Computes the gas cost of a transaction in USD.
///
/// cost_native = gas_limit * gas_price_gwei * 1e-9
/// cost_usd    = cost_native * native_usd
pub fn gas_cost_usd(gas_limit: u64, gas_price_gwei: f64, native_usd: f64) -> f64 {
    let cost_native = gas_limit as f64 * gas_price_gwei * 1e-9;
    cost_native * native_usd
}

/// Evaluates a round-trip arbitrage: buy the intermediate token on `buy` DEX,
/// sell it back on `sell` DEX, repay the flash loan + premium, pay gas.
///
/// `quotes` must contain, for the same `trade_size`:
///   - `mid_out`: intermediate tokens received from spending `trade_size` base on the buy DEX
///   - `base_out`: base tokens received from selling all `mid_out` on the sell DEX
pub fn evaluate(quotes: &PairQuotes, params: &EvalParams) -> ArbResult {
    let trade_size = quotes.trade_size;
    let gross_return = quotes.base_out;

    // Gross spread relative to principal.
    let spread_pct = if trade_size > 0.0 {
        (gross_return - trade_size) / trade_size
    } else {
        0.0
    };

    // Aave premium is charged on the borrowed principal.
    let premium_cost = trade_size * (params.premium_bps as f64 / 10_000.0);

    // Gas, expressed in USD then converted into base-token units.
    let gas_usd = gas_cost_usd(params.gas_limit, params.gas_price_gwei, params.native_usd);
    let gas_cost = if params.base_usd > 0.0 {
        gas_usd / params.base_usd
    } else {
        0.0
    };

    // Net = what we got back, minus principal, minus premium, minus gas.
    let net_profit = gross_return - trade_size - premium_cost - gas_cost;
    let profitable = net_profit >= params.min_profit;

    ArbResult {
        chain: quotes.chain.clone(),
        pair: quotes.pair.clone(),
        buy_dex: quotes.buy_dex.clone(),
        sell_dex: quotes.sell_dex.clone(),
        trade_size,
        gross_return,
        spread_pct,
        premium_cost,
        gas_cost,
        net_profit,
        profitable,
    }
}

/// Picks the most profitable result from a set of candidates, if any clears the bar.
pub fn best<'a>(results: &'a [ArbResult]) -> Option<&'a ArbResult> {
    results
        .iter()
        .filter(|r| r.profitable)
        .max_by(|a, b| {
            a.net_profit
                .partial_cmp(&b.net_profit)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quotes(trade: f64, base_out: f64) -> PairQuotes {
        PairQuotes {
            chain: "base".into(),
            pair: "USDC/WETH".into(),
            buy_dex: "uniswap_v3".into(),
            sell_dex: "aerodrome".into(),
            trade_size: trade,
            mid_out: 0.0,
            base_out,
        }
    }

    fn params() -> EvalParams {
        EvalParams {
            premium_bps: 5,         // 0.05%
            gas_limit: 450_000,
            gas_price_gwei: 0.05,   // cheap L2
            native_usd: 3_500.0,    // ETH
            base_usd: 1.0,          // base token is USDC
            min_profit: 25.0,
        }
    }

    #[test]
    fn gas_cost_matches_manual_calc() {
        // 450_000 * 0.05 gwei = 22_500 gwei = 2.25e-5 ETH * $3500 = $0.07875
        let usd = gas_cost_usd(450_000, 0.05, 3_500.0);
        assert!((usd - 0.07875).abs() < 1e-6, "got {usd}");
    }

    #[test]
    fn profitable_when_spread_beats_costs() {
        // Borrow 100k USDC, get back 100_600 -> 600 gross spread.
        let q = quotes(100_000.0, 100_600.0);
        let r = evaluate(&q, &params());
        // premium = 100k * 0.0005 = 50; gas ~ $0.08; net ~ 549.92
        assert!(r.profitable);
        assert!((r.premium_cost - 50.0).abs() < 1e-9);
        assert!(r.net_profit > 549.0 && r.net_profit < 550.0, "net={}", r.net_profit);
    }

    #[test]
    fn unprofitable_when_spread_too_thin() {
        // 100k -> 100_040: 40 gross < 50 premium alone.
        let q = quotes(100_000.0, 100_040.0);
        let r = evaluate(&q, &params());
        assert!(!r.profitable);
        assert!(r.net_profit < 0.0);
    }

    #[test]
    fn negative_spread_is_rejected() {
        let q = quotes(100_000.0, 99_500.0);
        let r = evaluate(&q, &params());
        assert!(!r.profitable);
        assert!(r.spread_pct < 0.0);
    }

    #[test]
    fn best_selects_highest_net() {
        let a = evaluate(&quotes(100_000.0, 100_200.0), &params());
        let b = evaluate(&quotes(100_000.0, 100_900.0), &params());
        let c = evaluate(&quotes(100_000.0, 100_010.0), &params()); // unprofitable
        let pick = best(&[a, b.clone(), c]).unwrap();
        assert!((pick.net_profit - b.net_profit).abs() < 1e-9);
    }
}
