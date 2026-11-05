---
name: whitechain-dev
description: >
  Whitechain developer playbook. Use for any task involving Whitechain development on testnet:
  connect to whitechain, add whitechain network to wallet, whitechain RPC url, whitechain chain ID,
  deploy contract to whitechain, compile solidity, forge deploy, deploy to whitechain testnet,
  create ERC-20 on whitechain, create NFT on whitechain, verify contract on whitechain,
  submit source code to blockscout, verify on blockscout, get testnet WBT, claim faucet,
  I need test tokens, run whitechain node, self-hosted RPC, run full node.
  Do not use this skill for transferring tokens, swapping, signing messages, or bridging —
  these are not yet supported (planned for a separate end-user MCP skill).
---

# Whitechain developer playbook

## Routing table

| Task | Trigger phrases | Reference |
|---|---|---|
| Connect wallet / network config | "connect to whitechain", "add network to wallet", "what's the RPC", "chain ID" | references/network.md |
| Deploy a contract | "deploy contract", "compile and deploy", "create ERC-20", "create NFT", "forge deploy" | references/deploy-contract.md |
| Verify a contract | "verify contract", "submit source code", "verify on blockscout" | references/verify-contract.md |
| Get testnet WBT | "get testnet WBT", "claim faucet", "I need test tokens" | references/claim-testnet-wbt.md |
| Run a node | "run whitechain node", "self-hosted RPC", "run full node" | references/run-node.md |

## Developer context

This skill assumes the developer controls their own signing key (Foundry keystore, environment
variable, or equivalent) and is running Claude Code or a terminal-capable agent. The agent
calls Foundry (`forge`/`cast`) and shell tools directly — no MCP approval flow is involved.

Transferring tokens, swapping, or bridging from a chat interface is out of scope for this
skill. A dedicated end-user MCP skill for those flows is planned but not yet built.

## Default network

| Property | Value |
| --- | --- |
| Name | Whitechain Sepolia |
| Chain ID | `1874` |
| RPC | `https://rpc.testnet.whitechain.io` |
| Explorer | `https://explorer.testnet.whitechain.io` |
| Blockscout API | `https://explorer.testnet.whitechain.io/api/v2` |
| Native token | WBT |

All operations target Whitechain Sepolia testnet.

## Safety guardrails

- **Never commit private keys or accept them pasted in chat** — use Foundry's encrypted keystore (`cast wallet import --interactive`); testnet key hygiene should be no different from production key hygiene
- **Always show a summary before any write operation** (contract call, deployment) — silent writes are irreversible once mined
- **Wait for explicit confirmation before signing** — do not infer confirmation from an unrelated follow-up message
- **Validate chain ID before signing** — confirm the wallet/RPC is on Whitechain Sepolia (`1874`) first; signing against the wrong chain risks replay on another network
- **Validate all user-provided shell inputs** before constructing `forge`/`cast`/`solc` commands — no spaces, semicolons, pipes, or backticks (see each reference's Input validation section)

## Operating procedure

1. **Classify the task** using the routing table above
2. **Read the relevant reference** before implementing — don't rely on memory of the API shape
3. **Validate inputs** per the reference's Input validation section before building any shell command
4. **Implement**, showing a summary and waiting for confirmation before any write or signing step
5. **Deliver** the result — transaction hash, explorer link, or returned value — plus any manual steps the user still needs to do (e.g. funding the signer)

## For edge cases and API changes

Blockscout's API and Whitechain's RPC can change without notice. If a reference's endpoint or
response shape stops matching reality:

- Check the live docs: https://docs.whitechain.io
- Check the Blockscout OpenAPI spec on the explorer itself: `https://explorer.testnet.whitechain.io/api-docs`
- Report the mismatch to the user rather than guessing at a fixed shape

## Installation

```
npx skills add whitechain-labs/skills --skill whitechain-dev
```