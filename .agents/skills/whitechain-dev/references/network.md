# Connecting to Whitechain Sepolia

## Network config

| Property | Value |
| --- | --- |
| Network name | Whitechain Sepolia |
| Chain ID | `1874` |
| RPC endpoint | `https://rpc.testnet.whitechain.io` |
| WebSocket | `wss://rpc.testnet.whitechain.io/ws` |
| Currency | WBT |
| Explorer | `https://explorer.testnet.whitechain.io` |
| Blockscout API | `https://explorer.testnet.whitechain.io/api/v2` |

This playbook only targets Whitechain Sepolia testnet — no other network is in scope.

## Security

- **Never embed the RPC endpoint behind an API key in client-side code** — proxy through a backend if a paid RPC provider is ever introduced
- **Validate the chain ID before signing** — a transaction signed against the wrong chain ID can be replayed on another chain
- **Use HTTPS RPC endpoints only** — reject any `http://` endpoint to prevent credential/data interception
- **The public RPC is rate-limited** — 50 rps sustained / burst 500 per IP; returns `429 Too Many Requests` with a `Retry-After: 1` header when exceeded; fine for development and testing, not a guarantee of uptime for anything time-sensitive

## Wallet setup

Three ways to add the network, fastest first:

**Option 1 — Chainlist (one click)**
1. Open [Chainlist, pre-filtered to Whitechain Sepolia](https://chainlist.org/?search=Whitechain+Sepolia&testnets=true)
2. Click **Connect Wallet**, then **Add to MetaMask**

**Option 2 — from the explorer**
1. Open the [Whitechain Testnet Explorer](https://explorer.testnet.whitechain.io/)
2. Click **Connect Wallet** in the top-right corner and approve the prompt

**Option 3 — manual**
1. In the wallet, choose **Add network** → **Add a network manually**
2. Enter the Chain ID, RPC endpoint, currency symbol, and explorer URL from the table above

After any option:
1. Confirm the wallet shows "Whitechain Sepolia" and chain ID `1874` before signing anything
2. Get test WBT from the faucet — see [claim-testnet-wbt.md](claim-testnet-wbt.md)

Full connect-wallet walkthrough: https://docs.whitechain.io/getting-started/quick-start/connect_wallet

## Using viem

```ts
import { defineChain } from 'viem'

export const whitechainSepolia = defineChain({
  id: 1874,
  name: 'Whitechain Sepolia',
  nativeCurrency: { name: 'WBT', symbol: 'WBT', decimals: 18 },
  rpcUrls: { default: { http: ['https://rpc.testnet.whitechain.io'] } },
  blockExplorers: { default: { name: 'Blockscout', url: 'https://explorer.testnet.whitechain.io' } },
})
```

## Error handling

| Error | Cause | Action |
| --- | --- | --- |
| Wallet shows wrong chain ID after adding network | Typo in Chain ID field | Re-check against the table above — `1874`, not `1337` or another testnet's ID. |
| RPC times out or refuses connection | Public RPC rate limit or outage | Retry after a short delay, or check https://status.whitechain.io/ for current uptime. |
| `429 Too Many Requests` response | Rate limit exceeded (50 rps sustained / burst 500 per IP) | Respect the `Retry-After: 1` header before retrying — each HTTP request counts against the limit, not each JSON-RPC call inside a batch. |
| Transaction signed but never appears on Whitechain Sepolia | Wallet was still on a different chain when signing | Re-confirm the active network in the wallet before retrying. |