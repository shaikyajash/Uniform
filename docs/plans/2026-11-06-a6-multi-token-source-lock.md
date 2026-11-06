# A6: Multi-token SourceLock and EURC pricing

Customers on Base Sepolia (and later Ethereum Sepolia) can pay with USDC or EURC. The SourceLock
holds the token as paid and never swaps, so a refund returns exactly what was locked and a failed
payment costs the customer no swap fee. The Matchmaker values EURC locks before filling.

## What was built
- **SourceLock v2** (`contracts/src/SourceLock.sol`): USDC and EURC fixed at deploy; gasless
  `lockWithAuthorization` for either (EIP-3009); a generic `lock` where the caller is the payer
  (replaces `lockFromProvider`, and is what the 7702 path in A12 uses); `claim` pays the receiver
  the locked token; `refund` returns the exact token and amount. `minUsdcOut` is enforced on chain
  for USDC locks only.
- **Matchmaker:** reads the new `Locked` event and `locks()` fields; values EURC at the CoinGecko
  price (fresh within 10 min, $0.90–$1.40) less `EURC_PRICE_BUFFER_BPS` (default 1%), rounded
  down; runs every other fill rule before asking for the price; checks Circle's blocklist on the
  locked token; refuses at start-up a SourceLock or token whose addresses or decimals differ from
  `chains.json`.
- **Shared:** the `Locked` report to the Order Desk carries `token` and `amount`.

## Deployed
SourceLock v2 on Base Sepolia: `0x02dF2659B5B4956EAFEe3293584C2aaefC114871`.

## Live EURC payment (Base Sepolia → Whitechain Sepolia)
A 1 USDT order paid with 0.91 EURC, with the Matchmaker on the public RPCs and a stand-in Order Desk:

| Step | Chain | Transaction |
|---|---|---|
| Bob ships a 2 USDT strategy on Aqua | Whitechain | [0x88a2a7d5…](https://explorer.testnet.whitechain.io/tx/0x88a2a7d5a04a8d0df2073ba82209666847e6d99e8a0dfc09794b52c647e07442) |
| Customer locks 0.91 EURC for Bob (signature only) | Base Sepolia | [0xc9fa0353…](https://sepolia.basescan.org/tx/0xc9fa0353517c614ffe74a5bc746d9be53748b787eb6e92b78393e8b4dcf70562) |
| **Matchmaker values the EURC and fills** the destination lock | Whitechain | [0x0d812d64…](https://explorer.testnet.whitechain.io/tx/0x0d812d64611a32838e4d0de08b2d710604d814d5e1bf1fbcef36df546b609223) |
| The secret is revealed: the shop gets 1 USDT (`Settled`) | Whitechain | [0x36eb70b5…](https://explorer.testnet.whitechain.io/tx/0x36eb70b51a83a837fbbd1af80249e22215afcf16a0124c489aeca070346b7cc6) |
| **Matchmaker redeems** the source lock: Bob gets 0.91 EURC | Base Sepolia | [0x8891f531…](https://sepolia.basescan.org/tx/0x8891f53128642d19975485585850556a455839ad41488bf0c0fdc3bb7d1e98f7) |

On local forks, a 0.80 EURC lock for the same order was skipped (worth 0.887 of the 1.003 USDC
needed).

## Later
- **At the end:** our EURC/USDC Uniswap v4 pool, Bob converting what he collects to USDC through
  the Universal Router, and the price check switching to the Uniswap Quoter.
- **With B's cross-chain quotes:** accept only the exact lock id recomputed from the quote, so a
  stranger's lock for a known order id cannot take the order's fill.
