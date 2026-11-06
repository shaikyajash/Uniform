# End-to-end test on local forks

Runs whole payments with the real Order Desk, Relayer and Notifier and A's Matchmaker, against
the deployed contracts on local anvil forks (Base Sepolia :8601, Whitechain :8602, Ethereum
Sepolia). Nothing is sent to a real network. Reuses `server/matchmaker/e2e/setup.sh` for the
forks, Bob's strategy and the test customer's USDC. The Relayer uses a fresh key funded on the
forks only. A stand-in shop server on :4199 (`webhook_receiver.py`) checks each webhook's
signature and logs it to `.state/webhooks.jsonl`.

```bash
cargo build -p order-desk -p relayer -p notifier -p matchmaker   # in server/
./start.sh                  # forks, a fresh uniform_e2e database, every service
./direct.sh 1 direct-1      # the shop signs in with its wallet and sets its webhook URL; a customer
                            # pays 1 USDT on Whitechain; the shop refunds 0.4, then the rest
./pay.sh 1 secret-1         # a 1 USDT order paid with USDC from Base: quote, sign, lock, fill, claim
./pay.sh 3 secret-2 nofill  # over Bob's 2 USDT limit: locked, never filled, refunded (run it on a
                            # fresh start: the test customer has 3 USDC, too little after pay.sh 1)
./stop.sh
```

Each script exits non-zero on the first thing that does not happen. Logs are in `.state/`.
