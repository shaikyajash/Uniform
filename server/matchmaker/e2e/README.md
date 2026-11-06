# Matchmaker end-to-end test on local forks

Runs a full cross-chain payment against the real deployed contracts on two local anvil forks
(Base Sepolia on :8601, Whitechain Sepolia on :8602), with a stand-in Order Desk on :8603.
Nothing is sent to a real network. Uses anvil's public test keys plus a fresh customer key kept in
`.state/` (ignored by git).

```bash
cargo build -p matchmaker            # in server/
./setup.sh                           # forks, mock Order Desk, funding, Bob's strategy
nohup ./run_matchmaker.sh > .state/matchmaker.log 2>&1 &
./lock.sh order-1 secret-1           # the customer locks 1.003 USDC for Bob; prints the source lock id
TOKEN=0x808456652fdb597867f38412077A9182bf77359F AMOUNT=905000 ./lock.sh order-2 secret-2   # or EURC
```

Then:
- **Normal payment:** reveal `cast keccak secret-1` on the DestinationLock (`claim(lockId, secret)`,
  with `lockId = lockIdFor(84532, sourceLockId, Bob)`). The Matchmaker redeems the source lock.
- **EURC:** an EURC lock is valued at the live CoinGecko price less 1%. For the 1 USDT order,
  about 0.905 EURC is filled and 0.80 EURC is skipped as `NotEnoughLocked`.
- **Timeout:** don't reveal; run `cast rpc evm_increaseTime 1260` and `cast rpc evm_mine` on both
  forks. The Matchmaker refunds both sides.
- **Crash recovery:** stop the Matchmaker after the fill, delete the fill row from
  `.state/matchmaker.db`, reveal the secret, restart. It records the fill from the chain and redeems.

Stop everything with `pkill -f "anvil --port 860"; pkill -f mock_order_desk.py; pkill -f target/debug/matchmaker`.
