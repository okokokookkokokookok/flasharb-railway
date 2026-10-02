//! Executor trigger: encodes an opportunity into an `ArbParams` payload and
//! (optionally) sends `executeArbitrage` to the on-chain `FlashArbExecutor`.
//!
//! By default, the bot runs in DRY-RUN mode and only logs what it *would* send.
//! Set `EXECUTE=1` in the environment and provide `PRIVATE_KEY` to arm it.

use std::sync::Arc;

use anyhow::{Context, Result};
use ethers::prelude::*;
use ethers::providers::{Http, Provider};
use ethers::signers::LocalWallet;

use crate::arb::ArbResult;
use crate::config::{ChainConfig, DexConfig, DexKind, PairConfig};
use crate::scanner::to_raw;

abigen!(
    FlashArbExecutor,
    r#"[
        {
            "inputs": [
                {
                    "internalType": "address",
                    "name": "loanAsset",
                    "type": "address"
                },
                {
                    "internalType": "uint256",
                    "name": "amount",
                    "type": "uint256"
                },
                {
                    "components": [
                        {
                            "internalType": "address",
                            "name": "midToken",
                            "type": "address"
                        },
                        {
                            "components": [
                                {
                                    "internalType": "enum FlashArbExecutor.DexKind",
                                    "name": "kind",
                                    "type": "uint8"
                                },
                                {
                                    "internalType": "address",
                                    "name": "router",
                                    "type": "address"
                                },
                                {
                                    "internalType": "uint24",
                                    "name": "fee",
                                    "type": "uint24"
                                }
                            ],
                            "internalType": "struct FlashArbExecutor.Dex",
                            "name": "buyDex",
                            "type": "tuple"
                        },
                        {
                            "components": [
                                {
                                    "internalType": "enum FlashArbExecutor.DexKind",
                                    "name": "kind",
                                    "type": "uint8"
                                },
                                {
                                    "internalType": "address",
                                    "name": "router",
                                    "type": "address"
                                },
                                {
                                    "internalType": "uint24",
                                    "name": "fee",
                                    "type": "uint24"
                                }
                            ],
                            "internalType": "struct FlashArbExecutor.Dex",
                            "name": "sellDex",
                            "type": "tuple"
                        },
                        {
                            "internalType": "uint256",
                            "name": "minProfit",
                            "type": "uint256"
                        }
                    ],
                    "internalType": "struct FlashArbExecutor.ArbParams",
                    "name": "params",
                    "type": "tuple"
                }
            ],
            "name": "executeArbitrage",
            "outputs": [],
            "stateMutability": "nonpayable",
            "type": "function"
        }
    ]"#
);

/// Maps the config's `DexKind` to the on-chain enum's `u8` discriminant.
fn dex_kind_u8(kind: DexKind) -> u8 {
    match kind {
        DexKind::UniswapV2 => 0,
        DexKind::UniswapV3 => 1,
    }
}

fn to_dex_tuple(dex: &DexConfig) -> Result<(u8, Address, u32)> {
    Ok((dex_kind_u8(dex.kind), dex.router.parse::<Address>()?, dex.fee))
}

/// Submits an arbitrage to the executor contract for a profitable opportunity.
///
/// `execute = false` performs a dry run: the calldata is built and logged but
/// never broadcast. This is the default and the recommended mode.
pub async fn trigger(
    chain: &ChainConfig,
    pair: &PairConfig,
    buy_dex: &DexConfig,
    sell_dex: &DexConfig,
    result: &ArbResult,
    execute: bool,
) -> Result<()> {
    let executor_addr = chain
        .executor
        .as_ref()
        .context("no executor address configured for chain")?
        .parse::<Address>()?;

    let loan_asset = pair.base.address.parse::<Address>()?;
    let mid_token = pair.quote.address.parse::<Address>()?;
    let amount = to_raw(pair.trade_size, pair.base.decimals);

    // Encode minProfit conservatively as 50% of simulated net profit to leave
    // headroom for slippage between simulation and execution.
    let min_profit = to_raw(result.net_profit * 0.5, pair.base.decimals);

    let params = ArbParams {
        mid_token,
        buy_dex: to_dex_params(buy_dex)?,
        sell_dex: to_dex_params(sell_dex)?,
        min_profit,
    };

    if !execute {
        tracing::info!(
            chain = %chain.name,
            pair = %result.pair,
            buy = %result.buy_dex,
            sell = %result.sell_dex,
            net_profit = result.net_profit,
            "DRY-RUN: would call executeArbitrage(loan={loan_asset:?}, amount={amount}, minProfit={min_profit})"
        );
        return Ok(());
    }

    // --- Live path: build a signing client and broadcast. ---
    let pk = std::env::var("PRIVATE_KEY").context("EXECUTE=1 but PRIVATE_KEY is unset")?;
    let provider = Provider::<Http>::try_from(chain.rpc_url.clone())?;
    let wallet = pk.parse::<LocalWallet>()?.with_chain_id(chain.chain_id);
    let client = Arc::new(SignerMiddleware::new(provider, wallet));

    let executor = FlashArbExecutor::new(executor_addr, client);

    let call = executor.execute_arbitrage(loan_asset, amount, params);
    let pending = call
        .send()
        .await
        .context("failed to broadcast executeArbitrage")?;

    tracing::info!(tx = ?pending.tx_hash(), "submitted arbitrage");
    Ok(())
}
