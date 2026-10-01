//! FLASHARB — cross-DEX arbitrage scanner funded by Aave V3 flash loans.
//!
//! The bot loops over configured chains, fetches round-trip DEX quotes for each
//! watched pair, simulates profitability after the flash-loan premium and gas,
//! and (in live mode) triggers the on-chain `FlashArbExecutor`.
//!
//! By default it runs in DRY-RUN: it scans and reports, but never broadcasts.

mod arb;
mod config;
mod executor;
mod scanner;

use std::time::Duration;

use anyhow::Result;
use tracing_subscriber::{fmt, EnvFilter};

use crate::arb::{evaluate, EvalParams};
use crate::config::Config;
use crate::scanner::Scanner;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env if present (RPC URLs, PRIVATE_KEY, etc.).
    let _ = dotenvy::dotenv();

    fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.toml".to_string());

    let mut cfg = Config::load(&config_path)?;
    cfg.resolve_env();

    let execute = std::env::var("EXECUTE").map(|v| v == "1").unwrap_or(false);
    if execute {
        tracing::warn!("EXECUTE=1 — bot is ARMED and will broadcast real transactions");
    } else {
        tracing::info!("running in DRY-RUN mode (set EXECUTE=1 to arm)");
    }

    tracing::info!(
        chains = cfg.chains.len(),
        min_profit_usd = cfg.min_profit_usd,
        poll_ms = cfg.poll_interval_ms,
        "FLASHARB scanner starting"
    );

    let poll = Duration::from_millis(cfg.poll_interval_ms);

    loop {
        if let Err(e) = scan_once(&cfg, execute).await {
            tracing::error!("scan cycle failed: {e:#}");
        }
        tokio::time::sleep(poll).await;
    }
}

/// Runs a single scan pass across all chains and pairs.
async fn scan_once(cfg: &Config, execute: bool) -> Result<()> {
    for chain in &cfg.chains {
        let scanner = match Scanner::new(chain.clone()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(chain = %chain.name, "failed to init scanner: {e:#}");
                continue;
            }
        };

        // Refresh gas price once per chain per cycle.
        let gas_price_gwei = scanner.gas_price_gwei().await;

        for pair in &chain.pairs {
            let quotes = scanner.scan_pair(pair).await;
            if quotes.is_empty() {
                continue;
            }

            let params = EvalParams {
                premium_bps: chain.flashloan_premium_bps,
                gas_limit: cfg.gas_limit,
                gas_price_gwei,
                native_usd: chain.native_usd,
                // Treat the base token's USD value as 1.0 when it's a stable;
                // for non-stable bases, configure native_usd / pricing upstream.
                base_usd: base_usd_hint(&pair.base.symbol),
                min_profit: usd_to_base(cfg.min_profit_usd, &pair.base.symbol),
            };

            let results: Vec<_> = quotes.iter().map(|q| evaluate(q, &params)).collect();

            // Log every candidate at debug; surface the winner at info.
            for r in &results {
                tracing::debug!(
                    chain = %r.chain,
                    pair = %r.pair,
                    route = format!("{}->{}", r.buy_dex, r.sell_dex),
                    spread_pct = format!("{:.4}%", r.spread_pct * 100.0),
                    net = r.net_profit,
                    "candidate"
                );
            }

            if let Some(best) = arb::best(&results) {
                tracing::info!(
                    chain = %best.chain,
                    pair = %best.pair,
                    route = format!("{} -> {}", best.buy_dex, best.sell_dex),
                    spread_pct = format!("{:.4}%", best.spread_pct * 100.0),
                    net_profit = best.net_profit,
                    "PROFITABLE opportunity"
                );

                // Resolve the concrete DEX configs for the winning route.
                let buy = chain.dexes.iter().find(|d| d.name == best.buy_dex);
                let sell = chain.dexes.iter().find(|d| d.name == best.sell_dex);
                if let (Some(buy), Some(sell)) = (buy, sell) {
                    if let Err(e) =
                        executor::trigger(chain, pair, buy, sell, best, execute).await
                    {
                        tracing::error!("trigger failed: {e:#}");
                    }
                }
            }
        }
    }
    Ok(())
}

/// Rough USD-per-whole-token hint for common base assets. Stables ~= $1.
/// Non-stables fall back to 1.0 and rely on `native_usd`-driven gas costing;
/// production deployments should wire a real price oracle here.
fn base_usd_hint(symbol: &str) -> f64 {
    match symbol.to_uppercase().as_str() {
        "USDC" | "USDT" | "DAI" | "USDBC" => 1.0,
        _ => 1.0,
    }
}

/// Converts a USD threshold into base-token units using the hint above.
fn usd_to_base(usd: f64, symbol: &str) -> f64 {
    let price = base_usd_hint(symbol);
    if price > 0.0 {
        usd / price
    } else {
        usd
    }
}
