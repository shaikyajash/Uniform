# Uniform

A checkout for businesses on Whitechain. Customers pay from the chain and stablecoin they already hold, and the shop always receives USDT on Whitechain, with receipts, refunds, webhooks and CSV export.

Built at ETHGlobal Mumbai 2026.

## Payment paths

- **Direct:** the customer already has USDT on Whitechain, signs one message, and the USDT goes straight to the shop. The customer pays no gas.
- **Cross-chain:** the customer pays EURC, USDC or ETH on Base Sepolia or Ethereum Sepolia. It is swapped to USDC on Uniswap and locked; a liquidity provider's USDT pays the shop on Whitechain through 1inch Aqua, and one secret completes both sides.
- **Split:** part from each chain, all or nothing.

## Repository layout

Contracts are Solidity (Foundry), the server is Rust (one Cargo workspace in `server/`), and the web apps are TypeScript (Next.js with Privy).

| Folder | Contents | Owner |
|---|---|---|
| `contracts/` | Foundry: DirectPay, CustomerLock, PayoutPool, ShopLock | A |
| `server/matchmaker/` | Watches chains, fills orders from liquidity, collects for the provider | A |
| `server/order-desk/`, `server/gas-payer/`, `server/notifier/` | API and database, transaction relayer, webhooks | B |
| `web/` | Pay Button, Shop Dashboard, Receipt page, Earn Page | B (Earn Page: A) |
| `demo-shop/` | Separate demo store using the widget | B |
| `shared/` | Chain config, ABIs, shared types (changed only by agreement) | A+B |
| `docs/` | SPEC.md, AI_USAGE.md | A+B |

## Status

Work in progress. Setup, architecture and current limitations are documented here as they are built.

## License

MIT, except `contracts/src/PayoutPool.sol` (Aqua Source License 1.1). Powered by Aqua, © Degensoft Ltd 2025.
