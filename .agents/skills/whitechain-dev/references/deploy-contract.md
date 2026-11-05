# Deploy a contract on Whitechain

Compile with Foundry, deploy to Whitechain Sepolia via an encrypted keystore, and optionally
verify on Blockscout in the same command.

## Prerequisites

1. A Foundry project (`forge init` if none exists) with the contract under `src/`
2. A signer imported into Foundry's encrypted keystore — **never pass a raw private key on the command line**
3. Test WBT in the signer's address — see [claim-testnet-wbt.md](claim-testnet-wbt.md)

## Security

- **Never commit private keys or accept them pasted in chat** — use Foundry's encrypted keystore (`cast wallet import --interactive`); testnet key hygiene should be no different from production
- **Never attempt to run keystore-signing commands from the agent's own shell** — `cast wallet import`, `cast wallet address --account <name>`, and `forge create --account` all require an interactive TTY for the password prompt, which the agent's shell tool cannot provide; hand the exact command to the user to run in their own terminal
- **Treat a dry run as a failed deploy, not a completed one** — without `--broadcast`, `forge create` only simulates and prints a transaction-shaped result while sending nothing on-chain
- **Never skip the deployment summary and confirmation step** — a mined deployment cannot be undone

## Input validation

Validate all user-provided values before constructing any shell command.

| Input | Rule |
| --- | --- |
| Contract path | Must match `^[a-zA-Z0-9_/.-]+\.sol:[a-zA-Z0-9_]+$` (`<path>:<contract-name>`) — reject paths containing spaces, semicolons, pipes, backticks, or `..` segments |
| Keystore account name | Must match `^[a-zA-Z0-9_-]+$` |
| Constructor arguments | If provided, must be valid hex strings or typed Solidity values — reject shell metacharacters |

If any input fails validation, stop and ask the user to correct it. Do not sanitize or rewrite invalid input.

## Steps

### 1. Confirm the target network

This skill deploys to Whitechain Sepolia testnet only. If the user asks for a different network,
tell them only testnet is supported in this version and stop.

### 2. Set up the signer

If the user has no keystore account yet, they need to create one — never accept a pasted private
key in chat. This command requires an interactive terminal prompt (for the private key and a
keystore password), which an agent's shell tool typically cannot provide — ask the user to run it
in their own terminal, then tell you the account name they chose:

```bash
cast wallet import <account-name> --interactive
```

Neither the private key nor the password is echoed, logged, or ever seen by the agent.

### 3. Compile

```bash
forge build
```

Fix and re-run on any compiler error before proceeding.

### 4. Show deployment summary and wait for confirmation

```
Deployment summary
───────────────────
Contract:  src/MyContract.sol:MyContract
Network:   Whitechain Sepolia (chain ID 1874)
Account:   <account-name>
Verify:    Yes (Blockscout)

Confirm? (yes / no)
```

### 5. Deploy

```bash
forge create src/MyContract.sol:MyContract \
  --rpc-url https://rpc.testnet.whitechain.io \
  --account <account-name> \
  --broadcast \
  --verify \
  --verifier blockscout \
  --verifier-url https://explorer.testnet.whitechain.io/api/
```

`--broadcast` is required — without it `forge create` only simulates the deployment ("Dry run
enabled, not broadcasting transaction") and sends nothing on-chain, while still printing a
transaction payload that looks like a real result.

This command needs the keystore password interactively, same as step 2 — ask the user to run it
in their own terminal and paste back the output (`Deployer:`, `Deployed to:`, `Transaction hash:`).

If the constructor takes arguments, add `--constructor-args <arg1> <arg2> ...` **as the last flag
in the command, after `--verifier-url`** — `--constructor-args` is variadic and greedily consumes
every token that follows it on the command line, so placing it earlier swallows `--verify` and the
rest as extra constructor arguments and fails with "Constructor argument count mismatch":

```bash
forge create src/MyToken.sol:MyToken \
  --rpc-url https://rpc.testnet.whitechain.io \
  --account <account-name> \
  --broadcast \
  --verify \
  --verifier blockscout \
  --verifier-url https://explorer.testnet.whitechain.io/api/ \
  --constructor-args 1000000000000000000000000
```

If `--verify` fails or the contract has multiple source files, fall back to a manual submission —
see [verify-contract.md](verify-contract.md).

### When to use `forge script` instead

`forge create` only covers "deploy one contract, nothing else." If the task needs multiple
contracts wired together or a post-deploy call (e.g. `initialize()`), use a `forge script` deploy
script (`vm.startBroadcast()` / `vm.stopBroadcast()`) instead — see the official
[Deploy with Foundry](https://docs.whitechain.io/getting-started/quick-start/deploy_with_foundry)
guide for the pattern. It also requires `--broadcast`, with the same silent-dry-run behavior.

### 6. Return the result

```
Deployed. Contract address: 0x...
Transaction: 0x...
Explorer: https://explorer.testnet.whitechain.io/address/0x...
```

## Error handling

| Error | Cause | Action |
| --- | --- | --- |
| `Device not configured (os error 6)` | Agent's shell has no TTY to prompt for the keystore password | Hand the exact command to the user to run in their own terminal — do not attempt to run keystore-signed commands directly. |
| "Dry run enabled, not broadcasting transaction" in the output | `--broadcast` was omitted | Nothing was deployed. Re-run the same command with `--broadcast` added. |
| Compilation error | Syntax error or wrong solc version pinned in `foundry.toml` | Show the compiler error. Ask the user to fix the source or the pinned version. |
| `insufficient funds for gas` | Signer address has no test WBT | Direct the user to [claim-testnet-wbt.md](claim-testnet-wbt.md). |
| `nonce too low` / `already used` | Concurrent deploy from the same account | Retry — `forge create` refetches the nonce. |
| Deployment reverted | Constructor error | Show the revert reason. |
| Verification fails via `--verify` | Verifier endpoint unreachable or compiler settings mismatch | Retry with manual submission — see [verify-contract.md](verify-contract.md). |
| `Constructor argument count mismatch: expected N but got M` | `--constructor-args` was placed before another flag (`--verify`, `--verifier`, etc.), which got swallowed as extra constructor arguments | Move `--constructor-args` to the very end of the command — it must be the last flag, since it greedily consumes every token after it. |
