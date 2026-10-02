# FLASHARB

Cross-DEX arbitrage engine: an off-chain Rust scanner paired with a Solidity executor that funds every trade with an **Aave V3 flash loan**. No working capital required — borrow, arb across two DEXes, repay principal + premium in the same transaction, keep the spread.

Built for **Base, Arbitrum, Optimism, and Polygon**. Supports Uniswap V2-style and Uniswap V3-style routers on either leg of the route.

> ⚠️ **Dry-run by default.** The bot scans and reports but never broadcasts unless you explicitly arm it with `EXECUTE=1`. Read the [Safety](#safety) section before going live.

---

## How it works

```
┌─────────────────┐      quotes        ┌──────────────────────┐
│  Rust scanner   │ ◄───────────────── │  DEXes (V2 / V3)     │
│  (off-chain)    │                    │  per chain           │
│                 │                    └──────────────────────┘
│  • round-trip   │
│    quote each   │   executeArbitrage()  ┌───────────────────────┐
│    pair         │ ─────────────────────►│  FlashArbExecutor.sol │
│  • net profit   │                       │  (on-chain)           │
│    after premium│                       │                       │
│    + gas        │      flashLoanSimple  │  1. borrow loanAsset  │
└─────────────────┘   ◄──────────────────┤  2. buy mid on DEX A  │
                          Aave V3 Pool    │  3. sell mid on DEX B │
                                          │  4. repay + premium   │
                                          │  5. sweep profit      │
                                          └───────────────────────┘
```

1. The scanner loops over each configured chain and watched pair, fetching round-trip quotes from every DEX.
2. It simulates net profit **after** the flash-loan premium (5 bps on Aave V3) and gas.
3. When a route clears `min_profit_usd`, the bot calls `executeArbitrage` on the deployed executor.
4. `FlashArbExecutor` borrows `loanAsset` via `flashLoanSimple`, swaps cheap→expensive across the two DEXes, and reverts unless it ends with `amount + premium + minProfit`.
5. Aave pulls repayment; the remainder is profit, withdrawable by the owner.

The on-chain `minProfit` guard means an unprofitable trade reverts atomically — you only ever pay gas, never principal.

---

## Repository layout

```
flasharb/
├── bot/                      Rust scanner + executor trigger
│   ├── Cargo.toml
│   ├── config.example.toml   chains, DEXes, pairs, addresses
│   └── src/
│       ├── main.rs           scan loop, dry-run / armed switch
│       ├── config.rs         TOML + ${ENV} resolution
│       ├── scanner.rs        on-chain DEX quoting
│       ├── arb.rs            profitability math
│       └── executor.rs       builds & sends the on-chain tx
└── contracts/                Foundry project
    ├── foundry.toml
    ├── src/
    │   ├── FlashArbExecutor.sol
    │   └── interfaces/        IPool, IFlashLoanSimpleReceiver, IERC20, routers
    ├── script/Deploy.s.sol
    └── test/FlashArbExecutor.t.sol
```

---

## Quick start

### 1. Contracts (Foundry)

```bash
cd contracts
forge install
forge build
forge test
```

Deploy to a target chain (Base shown):

```bash
cp ../.env.example ../.env   # fill PRIVATE_KEY + AAVE_ADDRESSES_PROVIDER
source ../.env

forge script script/Deploy.s.sol:Deploy \
  --rpc-url base \
  --broadcast \
  --verify
```

Note the deployed `FlashArbExecutor` address — you'll wire it into the bot config.

### 2. Bot (Rust)

```bash
cd bot
cp config.example.toml config.toml
# set `executor = "0x..."` under each chain you deployed to
cargo build --release
```

Dry-run (safe — scans and logs, broadcasts nothing):

```bash
cargo run --release -- config.toml
```

Arm it (broadcasts real transactions):

```bash
EXECUTE=1 cargo run --release -- config.toml
```

---

## Configuration

### Environment (`.env`)

| Variable | Purpose |
| --- | --- |
| `BASE_RPC_URL` / `ARBITRUM_RPC_URL` / `OPTIMISM_RPC_URL` / `POLYGON_RPC_URL` | Chain RPC endpoints (use your own Alchemy/Infura keys) |
| `PRIVATE_KEY` | Deploy + execution key — **never commit a real one** |
| `AAVE_ADDRESSES_PROVIDER` | Aave V3 `PoolAddressesProvider` for the chain you deploy to |
| `BASESCAN_API_KEY` etc. | Block-explorer keys for contract verification |
| `EXECUTE` | Set to `1` to arm the bot. Unset = dry-run |

### Scanner (`config.toml`)

Global knobs:

- `min_profit_usd` — ignore opportunities below this net profit (default `25.0`)
- `gas_limit` — expected gas for a full flash-loan arb tx (default `450000`)
- `poll_interval_ms` — delay between scan cycles (default `3000`)

Each `[[chains]]` block carries its `chain_id`, RPC (via `${ENV_VAR}`), Aave provider, `flashloan_premium_bps`, gas pricing, the deployed `executor` address, a list of `[[chains.dexes]]` (V2 or V3 routers + quoters), and the `[[chains.pairs]]` to watch. See `bot/config.example.toml` for fully populated Base / Arbitrum / Optimism / Polygon examples with real mainnet router and token addresses.

---

## Contract reference

`FlashArbExecutor` implements Aave's `IFlashLoanSimpleReceiver`.

- `executeArbitrage(address loanAsset, uint256 amount, ArbParams params)` — owner-only entrypoint; kicks off the flash loan.
- `executeOperation(...)` — Aave's mid-tx callback; runs both swap legs, enforces `minProfit`, approves repayment.
- `withdraw(address token)` — sweeps profit (or any stranded tokens) to the owner.
- `transferOwnership(address newOwner)` — standard ownership transfer.

`ArbParams` encodes the route: `midToken`, `buyDex`, `sellDex` (each a `{ kind, router, fee }` descriptor), and `minProfit`. The contract is intentionally permissioned — only the owner can initiate loans, since `executeOperation` runs arbitrary swaps.

---

## Safety

- **Dry-run is the default.** Nothing is broadcast unless `EXECUTE=1`.
- **Atomic downside.** The on-chain `minProfit` check reverts unprofitable trades — you lose gas, never principal.
- **Owner-gated.** Loans can only be initiated by the contract owner; `executeOperation` rejects calls that don't originate from the Aave Pool with `this` as initiator.
- **Keys.** `PRIVATE_KEY` lives only in your local `.env` (gitignored). Never commit it. Use a dedicated hot wallet with limited funds.
- **MEV.** Public-mempool arb is competitive and frequently front-run. Consider a private relay/bundle for live execution; the spreads you see in dry-run will not all be capturable.

---

## Tech stack

**Bot:** Rust 2021 · ethers 2.0 · tokio · serde/toml · anyhow · tracing
**Contracts:** Solidity 0.8.20 · Foundry · Aave V3 · Uniswap V2/V3 routers

## License

MIT © Loxee
