# Cross-chain end-to-end test on local forks

Runs a whole cross-chain payment with the real Order Desk, the real Relayer and A's Matchmaker,
against the deployed contracts on two local anvil forks (Base Sepolia :8601, Whitechain :8602).
Nothing is sent to a real network. Reuses `server/matchmaker/e2e/setup.sh` for the forks, Bob's
strategy and the test customer's USDC. The Relayer uses a fresh key funded on the forks only.

```bash
cargo build -p order-desk -p relayer -p matchmaker   # in server/
./start.sh                  # forks, a fresh uniform_e2e database, Order Desk :4100, Relayer, Matchmaker
./pay.sh 1 secret-1         # a 1 USDT order paid with USDC from Base: quote, sign, lock, fill, claim
./pay.sh 3 secret-2 nofill  # a 3 USDT order is over Bob's 2 USDT limit: locked, never filled, refunded
./stop.sh
```

Logs are in `.state/`.
