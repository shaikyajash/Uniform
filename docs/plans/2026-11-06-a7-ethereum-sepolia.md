# A7: Ethereum Sepolia as a second customer chain

Customers can now pay from Ethereum Sepolia as well as Base Sepolia. The SourceLock is the same
contract as on Base; the Matchmaker watches every customer chain with a `sourceLock` in
`shared/chains.json`.

## What changed
- **SourceLock** deployed on Ethereum Sepolia at `0x3aCDE874Eeb36dD28507B5b1FF29219Ba262F5d9`
  (USDC and EURC). Ethereum Sepolia charges far more gas for creating contracts and storage than
  forge's local EVM expects (8.9M gas for this deploy, 1.3M on Base), so scripts there run with
  `--skip-simulation` and use the node's own estimate.
- **Fill wait per chain:** `confirmations` in chains.json (Base 10 blocks, Ethereum 3), overridable
  with `CONFIRMATIONS_<chainId>`; `SAFE_TAG_CHAINS` optionally reads a rollup only up to its `safe`
  block. Defaults are tuned for a fast demo; README.md explains the trade-off.
- **Filler gas floor per chain:** 0.002 ETH on Ethereum, 0.0001 ETH elsewhere
  (`MIN_FILLER_GAS_WEI_<chainId>`).
- **Start-up checks:** a sourceLock with no code, or `SAFE_TAG_CHAINS` naming a non-customer
  chain, stops the Matchmaker; a rollup whose safe block cannot be read skips the pass.
- **Catch-up:** each pass reads up to 20 log ranges per chain, so a backlog clears at once.
- **Harness:** an Ethereum Sepolia fork on :8604; `CHAIN=ethereum ./lock.sh …`.

## Live payment (Ethereum Sepolia → Whitechain Sepolia)
| Step | Chain | Transaction |
|---|---|---|
| Deploy SourceLock | Ethereum Sepolia | [0x4bc9e3ff…](https://sepolia.etherscan.io/tx/0x4bc9e3ffdad7e97682f0a3106b9771d5c55ad26f2a2ed9f7a4066c778669dea9) |
| Customer locks 1.003 USDC for Bob (signature only) | Ethereum Sepolia | [0x28efc04d…](https://sepolia.etherscan.io/tx/0x28efc04d7df98c38ffc701e1f8ac05f6a7c14d1e625a2b9de087aef3e8b2f8d1) |
| **Matchmaker fills** the destination lock | Whitechain | [0xfc42d29c…](https://explorer.testnet.whitechain.io/tx/0xfc42d29c4af1d7c914e61cb79585869f2b1942e559d6763223b030c87c2a04cf) |
| The secret is revealed: the shop gets 1 USDT | Whitechain | [0xcd52e636…](https://explorer.testnet.whitechain.io/tx/0xcd52e6364ae147603c5afad98b2d556347fed080e21eeec93951a5030be76772) |
| **Matchmaker redeems** 1.003 USDC for Bob, 20 s later | Ethereum Sepolia | [0x3e304874…](https://sepolia.etherscan.io/tx/0x3e304874c9b92ad72ce7a8115ac75c272a9dac918d4404d98a91ff44710e9175) |

On local forks a timeout refunded both sides: Bob's USDT went back to his strategy and the
customer's USDC came back on Ethereum.

## Open
- Locks Bob never filled (too small, unknown order) are refunded by no service yet; anyone can
  call `refund` after expiry, but customers hold no gas.
- Stuck Ethereum transactions are not re-sent with a higher fee.
