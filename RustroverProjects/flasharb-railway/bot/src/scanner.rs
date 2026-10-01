//! Price fetching across DEX venues.
//!
//! For each configured pair we ask two DEXes for quotes and assemble a
//! round-trip simulation:
//!   1. Spend `trade_size` of `base` on DEX A  -> receive `mid_out` of `quote`.
//!   2. Spend `mid_out` of `quote` on DEX B    -> receive `base_out` of `base`.
//!
//! V2 venues are quoted with `getAmountsOut`; V3 venues with `QuoterV2`'s
//! `quoteExactInputSingle` (called read-only via `eth_call`).

use std::sync::Arc;

use anyhow::{Context, Result};
use ethers::prelude::*;
use ethers::providers::{Http, Provider};

use crate::config::{ChainConfig, DexConfig, DexKind, PairConfig};

// ---------------------------------------------------------------------------
// ABI bindings (generated at compile time from inline JSON fragments).
// ---------------------------------------------------------------------------

abigen!(
    UniswapV2Router,
    r#"[
        function getAmountsOut(uint256 amountIn, address[] path) external view returns (uint256[] memory)
    ]"#
);

abigen!(
    UniswapV3Quoter,
    r#"[
        struct QuoteExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint256 amountIn;
            uint24 fee;
            uint160 sqrtPriceLimitX96;
        }

        function quoteExactInputSingle(
            QuoteExactInputSingleParams params
        ) external returns (
            uint256 amountOut,
            uint160 sqrtPriceX96After,
            uint32 initializedTicksCrossed,
            uint256 gasEstimate
        )
    ]"#
);

/// A fully-assembled round-trip quote for one (pair, buyDex, sellDex) candidate.
#[derive(Debug, Clone)]
pub struct PairQuotes {
    pub chain: String,
    pub pair: String,
    pub buy_dex: String,
    pub sell_dex: String,
    /// Flash-loan size in base-token whole units.
    pub trade_size: f64,
    /// Intermediate tokens received on the buy leg (whole units).
    pub mid_out: f64,
    /// Base tokens received on the sell leg (whole units).
    pub base_out: f64,
}

/// Thin wrapper around an HTTP JSON-RPC provider for one chain.
pub struct Scanner {
    pub chain: ChainConfig,
    provider: Arc<Provider<Http>>,
}

impl Scanner {
    pub fn new(chain: ChainConfig) -> Result<Self> {
        let provider = Provider::<Http>::try_from(chain.rpc_url.clone())
            .with_context(|| format!("invalid RPC URL for chain {}", chain.name))?;
        Ok(Self {
            chain,
            provider: Arc::new(provider),
        })
    }

    /// Quotes a single exact-input swap `amount_in` of `token_in` -> `token_out`
    /// on `dex`, returning the raw output amount (token_out's smallest unit).
    async fn quote_swap(
        &self,
        dex: &DexConfig,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
    ) -> Result<U256> {
        match dex.kind {
            DexKind::UniswapV2 => {
                let router = UniswapV2Router::new(
                    dex.router.parse::<Address>()?,
                    self.provider.clone(),
                );
                let amounts: Vec<U256> = router
                    .get_amounts_out(amount_in, vec![token_in, token_out])
                    .call()
                    .await
                    .with_context(|| format!("getAmountsOut failed on {}", dex.name))?;
                amounts
                    .last()
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("empty amounts from {}", dex.name))
            }
            DexKind::UniswapV3 => {
                let quoter = UniswapV3Quoter::new(
                    dex.quoter.parse::<Address>()?,
                    self.provider.clone(),
                );
                let params = QuoteExactInputSingleParams {
                    token_in,
                    token_out,
                    amount_in,
                    fee: dex.fee,
                    sqrt_price_limit_x96: U256::zero(),
                };
                // QuoterV2 is non-view by signature but safe to eth_call.
                let (amount_out, _, _, _) = quoter
                    .quote_exact_input_single(params)
                    .call()
                    .await
                    .with_context(|| format!("quoteExactInputSingle failed on {}", dex.name))?;
                Ok(amount_out)
            }
        }
    }

    /// Builds a round-trip quote: buy `quote` token on `buy_dex`, then sell it
    /// back to `base` on `sell_dex`.
    pub async fn round_trip(
        &self,
        pair: &PairConfig,
        buy_dex: &DexConfig,
        sell_dex: &DexConfig,
    ) -> Result<PairQuotes> {
        let base = pair.base.address.parse::<Address>()?;
        let quote = pair.quote.address.parse::<Address>()?;

        let amount_in = to_raw(pair.trade_size, pair.base.decimals);

        // Leg 1: base -> quote on the buy DEX.
        let mid_raw = self.quote_swap(buy_dex, base, quote, amount_in).await?;

        // Leg 2: quote -> base on the sell DEX, feeding the full leg-1 output.
        let base_raw = self.quote_swap(sell_dex, quote, base, mid_raw).await?;

        Ok(PairQuotes {
            chain: self.chain.name.clone(),
            pair: format!("{}/{}", pair.base.symbol, pair.quote.symbol),
            buy_dex: buy_dex.name.clone(),
            sell_dex: sell_dex.name.clone(),
            trade_size: pair.trade_size,
            mid_out: from_raw(mid_raw, pair.quote.decimals),
            base_out: from_raw(base_raw, pair.base.decimals),
        })
    }

    /// Enumerates every ordered DEX pair (A buy / B sell) for a token pair and
    /// quotes each round trip. Failed quotes are logged and skipped so one bad
    /// venue doesn't sink the whole scan.
    pub async fn scan_pair(&self, pair: &PairConfig) -> Vec<PairQuotes> {
        let mut out = Vec::new();
        let dexes = &self.chain.dexes;
        for (i, buy) in dexes.iter().enumerate() {
            for (j, sell) in dexes.iter().enumerate() {
                if i == j {
                    continue;
                }
                match self.round_trip(pair, buy, sell).await {
                    Ok(q) => out.push(q),
                    Err(e) => tracing::debug!(
                        chain = %self.chain.name,
                        buy = %buy.name,
                        sell = %sell.name,
                        "quote failed: {e:#}"
                    ),
                }
            }
        }
        out
    }

    /// Fetches the current gas price (gwei) from the node, falling back to the
    /// configured assumption if the RPC call fails.
    pub async fn gas_price_gwei(&self) -> f64 {
        match self.provider.get_gas_price().await {
            Ok(wei) => wei.as_u128() as f64 / 1e9,
            Err(_) => self.chain.gas_price_gwei,
        }
    }
}

/// Converts a whole-unit amount into the token's smallest unit (e.g. wei).
pub fn to_raw(whole: f64, decimals: u8) -> U256 {
    // Use a string-based parse to avoid f64 precision loss on large notionals.
    let scaled = format!("{:.0}", whole * 10f64.powi(decimals as i32));
    U256::from_dec_str(&scaled).unwrap_or_else(|_| U256::zero())
}

/// Converts a smallest-unit amount back into whole token units as f64.
pub fn from_raw(raw: U256, decimals: u8) -> f64 {
    let divisor = 10f64.powi(decimals as i32);
    // U256 -> f64 via lossy string path keeps us correct across the L2 ranges we care about.
    let as_f64 = raw.to_string().parse::<f64>().unwrap_or(0.0);
    as_f64 / divisor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_roundtrip_6_decimals() {
        let raw = to_raw(100_000.0, 6);
        assert_eq!(raw, U256::from(100_000u64) * U256::exp10(6));
        assert!((from_raw(raw, 6) - 100_000.0).abs() < 1e-6);
    }

    #[test]
    fn raw_roundtrip_18_decimals() {
        let raw = to_raw(1.5, 18);
        assert!((from_raw(raw, 18) - 1.5).abs() < 1e-9);
    }
}
