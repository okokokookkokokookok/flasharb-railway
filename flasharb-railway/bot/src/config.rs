//! Chain, DEX and token-pair configuration for the arbitrage scanner.
//!
//! Configuration is loaded from a TOML file (see `config.example.toml`). Each
//! chain carries an RPC URL, the Aave V3 addresses-provider, the deployed
//! executor address, and a list of DEX venues + token pairs to watch.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Router semantics. Mirrors the on-chain `DexKind` enum so payloads line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DexKind {
    #[serde(rename = "uniswap_v2")]
    UniswapV2,
    #[serde(rename = "uniswap_v3")]
    UniswapV3,
}

/// A DEX venue on a given chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DexConfig {
    /// Human-readable name, e.g. "uniswap_v3", "sushiswap", "aerodrome".
    pub name: String,
    pub kind: DexKind,
    /// Router address used for swaps.
    pub router: String,
    /// Quoter address. For V2 this is usually the router (getAmountsOut);
    /// for V3 it is the QuoterV2 contract.
    pub quoter: String,
    /// Fee tier in hundredths of a bip (V3 only): 500 / 3000 / 10000.
    #[serde(default)]
    pub fee: u32,
}

/// A token with its on-chain address and decimals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub symbol: String,
    pub address: String,
    pub decimals: u8,
}

/// A pair to scan: `base` is the flash-loan asset (and profit denomination),
/// `quote` is the intermediate token routed through.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairConfig {
    pub base: Token,
    pub quote: Token,
    /// Notional size of the flash loan to simulate, in `base` whole units.
    pub trade_size: f64,
}

/// Per-chain configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainConfig {
    pub name: String,
    pub chain_id: u64,
    pub rpc_url: String,
    /// Aave V3 PoolAddressesProvider.
    pub aave_addresses_provider: String,
    /// Aave V3 flash loan premium in basis points (typically 5 = 0.05%).
    #[serde(default = "default_premium_bps")]
    pub flashloan_premium_bps: u32,
    /// Deployed `FlashArbExecutor` address (optional until deployed).
    #[serde(default)]
    pub executor: Option<String>,
    /// Native gas price assumption in gwei, used for cost simulation.
    #[serde(default = "default_gas_gwei")]
    pub gas_price_gwei: f64,
    /// USD price of the chain's native token, for converting gas to USD.
    #[serde(default)]
    pub native_usd: f64,
    pub dexes: Vec<DexConfig>,
    pub pairs: Vec<PairConfig>,
}

fn default_premium_bps() -> u32 {
    5
}

fn default_gas_gwei() -> f64 {
    0.05
}

/// Top-level bot configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Minimum net profit (in `base` token units) required to flag an opportunity.
    #[serde(default = "default_min_profit_usd")]
    pub min_profit_usd: f64,
    /// Gas units a full flash-loan arbitrage tx is expected to consume.
    #[serde(default = "default_gas_limit")]
    pub gas_limit: u64,
    /// Poll interval between scans, in milliseconds.
    #[serde(default = "default_poll_ms")]
    pub poll_interval_ms: u64,
    pub chains: Vec<ChainConfig>,
}

fn default_min_profit_usd() -> f64 {
    25.0
}

fn default_gas_limit() -> u64 {
    450_000
}

fn default_poll_ms() -> u64 {
    3_000
}

impl Config {
    /// Loads and validates configuration from a TOML file.
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path.as_ref())
            .map_err(|e| anyhow::anyhow!("failed to read config {}: {e}", path.as_ref().display()))?;
        let cfg: Config = toml::from_str(&raw)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Resolves `${ENV_VAR}` placeholders in RPC URLs from the process env.
    /// Lets the committed config reference secrets without embedding them.
    pub fn resolve_env(&mut self) {
        for chain in &mut self.chains {
            if let Some(var) = chain
                .rpc_url
                .strip_prefix("${")
                .and_then(|s| s.strip_suffix('}'))
            {
                if let Ok(val) = std::env::var(var) {
                    chain.rpc_url = val;
                }
            }
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.chains.is_empty() {
            anyhow::bail!("config has no chains");
        }
        for c in &self.chains {
            if c.dexes.len() < 2 {
                anyhow::bail!(
                    "chain {} needs at least 2 DEXes to arb, found {}",
                    c.name,
                    c.dexes.len()
                );
            }
            if c.pairs.is_empty() {
                anyhow::bail!("chain {} has no pairs to scan", c.name);
            }
        }
        Ok(())
    }

    /// Index chains by id for quick lookup.
    pub fn chains_by_id(&self) -> HashMap<u64, &ChainConfig> {
        self.chains.iter().map(|c| (c.chain_id, c)).collect()
    }
}
