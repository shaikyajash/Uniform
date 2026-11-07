# Uniform

A checkout for businesses on Whitechain. Customers pay from the chain and stablecoin they already hold, and the merchant always receives USDT on Whitechain, with receipts, refunds, webhooks and CSV export.

Built at ETHGlobal Mumbai 2026.

## Payment paths

- **Direct:** the customer already has USDT on Whitechain, signs one message, and the USDT goes straight to the merchant. The customer pays no gas.
- **Cross-chain:** the customer pays EURC, USDC or ETH on Base Sepolia or Ethereum Sepolia. It is locked as is, and swapped to USDC on Uniswap only when the payment succeeds, so a failed payment refunds the same token with no swap fee; a liquidity provider's USDT pays the merchant on Whitechain through 1inch Aqua, and one secret completes both sides.
- **Split:** part from each chain, all or nothing.

## Repository layout

Contracts are Solidity (Foundry), the server is Rust (one Cargo workspace in `server/`), and the web apps are TypeScript (Next.js with Privy).

| Folder | Contents | Owner |
|---|---|---|
| `contracts/` | Foundry: DirectPay, SourceLock, PayoutPool, DestinationLock | A |
| `server/matchmaker/` | Watches chains, fills orders from liquidity, collects for the provider | A |
| `server/order-desk/`, `server/gas-payer/`, `server/notifier/` | API and database, transaction relayer, webhooks | B |
| `web/` | Pay Button, Merchant Dashboard, Receipt page, Earn Page | B (Earn Page: A) |
| `demo-shop/` | Separate demo store using the widget | B |
| `shared/` | Chain config, ABIs, shared types (changed only by agreement) | A+B |
| `docs/` | SPEC.md, AI_USAGE.md | A+B |

## Matchmaker: fill speed vs. reorg safety

Before Bob pays a merchant, the Matchmaker waits until the customer's source lock is unlikely to be
undone by a reorg. A longer wait is safer for Bob; a shorter one makes payments feel instant.
**The defaults are tuned for a fast demo:**

| Chain | Default wait | About | Set with |
|---|---|---|---|
| Base Sepolia | 10 blocks | 2 s | `CONFIRMATIONS_84532` |
| Ethereum Sepolia | 3 blocks | 36 s | `CONFIRMATIONS_11155111` |
| Whitechain Sepolia | 1 block | 1 s | `CONFIRMATIONS_1874` |

The defaults come from `confirmations` in `shared/chains.json`; the environment overrides them.

**Safer settings (production):** set `SAFE_TAG_CHAINS=84532` so Base is read only up to its
`safe` block (adds 50 s to 2 min per fill), and raise `CONFIRMATIONS_11155111` to 12 or more
(about 2.5 min). Do not put Ethereum in `SAFE_TAG_CHAINS`: its `safe` block trails by about 9
minutes, longer than a source lock leaves to fill.

**The risk with the fast defaults:** if a reorg drops a customer's lock after a provider filled
it, the provider loses that order's USDT, at most their strategy's per-order limit. Keep that limit
small while running fast.

**Providers:** anyone who ships a strategy on the Earn page (USDT, Uniform's filler, a fee within
`providerFeeBpsRange` in chains.json). The Matchmaker sends every open strategy's offer to the
Order Desk every 15 s, and each cross-chain quote goes to the lowest fee that can cover the order
(of equal fees, the most free liquidity). The customer's lock names that provider, so their tokens
go to the provider who paid the shop.

Other Matchmaker settings: `POLL_SECONDS` (default 2), `MAX_BLOCK_RANGE` (default 1000),
`EURC_PRICE_BUFFER_BPS` (default 100 = 1%), and `MIN_FILLER_GAS_WEI` /
`MIN_FILLER_GAS_WEI_<chainId>` (gas the filler must hold on a source chain before it fills;
defaults 0.0001 ETH, 0.002 ETH on Ethereum).

## Status

Work in progress. Setup, architecture and current limitations are documented here as they are built.

## License

MIT, except `contracts/src/PayoutPool.sol` (Aqua Source License 1.1). Powered by Aqua, © Degensoft Ltd 2025.
