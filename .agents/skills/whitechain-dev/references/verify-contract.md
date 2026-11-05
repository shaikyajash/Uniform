# Verify a contract on Whitechain Blockscout

Submit Solidity source code for verification on the Blockscout explorer.

## Input validation

Validate all user-provided values before constructing the verification request.

| Input | Rule |
| --- | --- |
| Contract address | Must match `^0x[a-fA-F0-9]{40}$` |
| Contract name | Must match `^[a-zA-Z_][a-zA-Z0-9_]*$` — reject spaces, semicolons, pipes, or backticks |
| Compiler version | Must match `^v\d+\.\d+\.\d+\+commit\.[a-f0-9]+$` |
| Constructor arguments | If provided, must be a valid hex string (`^(0x)?[a-fA-F0-9]*$`) |

If any input fails validation, stop and ask the user to correct it. Do not sanitize or rewrite invalid input.

## Steps

### 1. Collect required inputs

| Input | Description |
| --- | --- |
| Contract address | Deployed address on Whitechain |
| Contract name | Exact contract name as declared in the source (e.g. `Storage`) |
| Solidity source | Full source file(s) |
| Compiler version | Must match the version used at deploy (e.g. `v0.8.28`) |
| Optimization | Whether optimization was enabled and the runs value |
| Constructor arguments | ABI-encoded, if the constructor takes arguments |

### 2. Submit for verification

```bash
curl -X POST https://explorer.testnet.whitechain.io/api/v2/smart-contracts/{address}/verification/via/flattened-code \
  -H 'Content-Type: application/json' \
  -d '{
    "contract_name": "Storage",
    "compiler_version": "v0.8.28+commit.7893614a",
    "source_code": "<flattened source>",
    "is_optimization_enabled": true,
    "optimization_runs": 200,
    "constructor_args": ""
  }'
```

`contract_name` is required — omitting it returns `400 Bad request` with no further detail.

For multi-file contracts, use the `standard-input` method instead of `flattened-code`. The full
list of supported methods is at `GET /api/v2/smart-contracts/verification/config`
(`verification_options`).

### 3. Poll for result

```
GET https://explorer.testnet.whitechain.io/api/v2/smart-contracts/{address}
```

Check `is_verified`. Blockscout typically verifies within 30 seconds.

### 4. Return the result

```
Verified. Contract source is now public.
Explorer: https://explorer.testnet.whitechain.io/address/0x.../contracts
```

## Proxy contracts

For proxy contracts (EIP-1967, transparent, UUPS), verify the implementation contract using the
steps above. Blockscout auto-detects the proxy pattern and links the proxy address to the
verified implementation — there is no separate proxy verification request on this Blockscout
version. Confirm via `GET /api/v2/addresses/{proxy_address}`: once linked, `proxy_type` and
`implementations` are populated automatically.

## Error handling

| Error | Cause | Action |
| --- | --- | --- |
| Bytecode mismatch | Wrong compiler version or optimization settings | Check `foundry.toml` or `hardhat.config` for the exact settings used at deploy. |
| Constructor args mismatch | Wrong ABI encoding | Re-encode using `cast abi-encode "constructor(type)" value`. |
| Already verified | Contract was verified before | Report the existing verification link. |