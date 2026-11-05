# Claim testnet WBT

Guide the user through claiming test WBT from the Whitechain Sepolia faucet.

The faucet requires GitHub OAuth and a captcha. This skill cannot claim autonomously.

## Input validation

| Input | Rule |
| --- | --- |
| Wallet address | Must match `^0x[a-fA-F0-9]{40}$` — reject anything else before constructing the API request |

If the address fails validation, stop and ask the user to correct it.

## Steps

### 1. Check the wallet is on Whitechain Sepolia

Confirm the user has Whitechain Sepolia (chain ID `1874`) configured in their wallet.
If not, direct them to https://docs.whitechain.io/getting-started/quick-start/connect_wallet.

### 2. Check the current balance

```
GET https://explorer.testnet.whitechain.io/api/v2/addresses/{address}
```

Report the current WBT balance before the claim.

### 3. Direct the user to the faucet

```
Open https://faucet.testnet.whitechain.io

Steps:
1. Sign in with GitHub (OAuth required — account must be at least 30 days old).
2. Paste your Whitechain address.
3. Complete the captcha.
4. Submit.
```

Drip is **0.5 WBT per 24-hour window** — the window is rolling from the moment of the last claim,
not a fixed daily reset. A claim at 14:00 today unlocks the next claim at 14:00 tomorrow.

### 4. Confirm the balance increased

After the user confirms they submitted the request, poll the Blockscout API every 10 seconds
for up to 2 minutes:

```
GET https://explorer.testnet.whitechain.io/api/v2/addresses/{address}
```

Compare against the balance from step 2 (treat a `null` starting balance as `0`). When
`coin_balance` increases, report success:

```
Test WBT received.
New balance: 0.5 WBT
Explorer: https://explorer.testnet.whitechain.io/address/0x...
```

## Notes

- GitHub account must be at least 30 days old to claim.
- Drip is 0.5 WBT per rolling 24-hour window.
- The faucet checks three limits **independently**: wallet address, IP address, and GitHub
  account. A claim can be rejected on IP grounds even on a fresh address/account (e.g. shared
  corporate network or VPN).
- Only works on Whitechain Sepolia testnet.

## Error handling

| Error | Cause | Action |
| --- | --- | --- |
| Balance unchanged after 2 min | Faucet delay, or the claim landed but the user hasn't submitted yet | Ask the user to check the faucet page for an error message and retry. |
| Claim rejected, address never claimed before | IP-address limit or GitHub-account limit hit, not the wallet's own cooldown | Ask if others on the same network/VPN claimed recently, or if the GitHub account is under 30 days old. |
| Cooldown active on a previously-claimed address | Rolling 24h window not yet elapsed | Report the window is rolling (not midnight reset) — compute next-eligible time from the last claim. |
| GitHub OAuth fails | Browser issue or Cloudflare captcha | Suggest trying a different browser, disabling VPN/ad-blockers, or clearing cookies for the faucet domain. |