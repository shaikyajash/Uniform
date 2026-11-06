#!/usr/bin/env bash
# Starts two local forks (Base Sepolia :8601, Whitechain :8602) and the mock Order Desk (:8603),
# then funds the test roles with impersonated transfers and ships Bob's strategy on the fork.
# Needs anvil, cast and python3. The deployer address only needs real balances to copy from.
# Uses anvil's well-known test keys only. Nothing is sent to a real network.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p .state
# The customer is a fresh key: anvil's well-known keys carry EIP-7702 code on Base Sepolia,
# and USDC then treats them as smart wallets and rejects plain signatures.
[ -f .state/customer.key ] || { cast wallet new --json | python3 -c "import json,sys; d=json.load(sys.stdin); d=d.get('data',d) if isinstance(d,dict) else d; d=d[0] if isinstance(d,list) else d; print(d['private_key'])" > .state/customer.key; chmod 600 .state/customer.key; }

BASE=http://127.0.0.1:8601
WC=http://127.0.0.1:8602
DEPLOYER=0x7801aE8881A59F231fA31D7a5b56cb20766fd2eB
USDC=0x036CbD53842c5426634e7929541eC2318f3dCF7e
EURC=0x808456652fdb597867f38412077A9182bf77359F
USDT=0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7
AQUA=0x02dF2659B5B4956EAFEe3293584C2aaefC114871
POOL=0x3D79F7D2366D26c7325F58bEADbf7ECcB1d13E01

# anvil test accounts
FILLER=0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
BOB=0x70997970C51812dc3A010C7d01b50e0d17dc79C8
BOB_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
CUSTOMER=$(cast wallet address $(cat .state/customer.key))
SALT=0x00000000000000000000000000000000000000000000000000000000000e2e01

pkill -f "anvil --port 860" 2>/dev/null || true
pkill -f mock_order_desk.py 2>/dev/null || true
rm -f .state/events.log .state/matchmaker.db .state/anvil-*.log; echo '{}' > .state/orders.json
sleep 1

nohup anvil --port 8601 --fork-url https://sepolia.base.org --block-time 1 >.state/anvil-base.log 2>&1 &
nohup anvil --port 8602 --fork-url https://rpc.testnet.whitechain.io --block-time 1 >.state/anvil-wc.log 2>&1 &
nohup python3 mock_order_desk.py 8603 >.state/mock.log 2>&1 &
for url in $BASE $WC; do
  for _ in $(seq 1 60); do cast chain-id --rpc-url $url >/dev/null 2>&1 && break; sleep 1; done
done
echo "forks up: base=$(cast chain-id --rpc-url $BASE) whitechain=$(cast chain-id --rpc-url $WC)"

# Whitechain: give Bob 5 USDT, then Bob approves Aqua and ships a 2 USDT strategy with the filler.
cast rpc anvil_impersonateAccount $DEPLOYER --rpc-url $WC >/dev/null
cast send $USDT "transfer(address,uint256)" $BOB 5000000 --from $DEPLOYER --unlocked --rpc-url $WC >/dev/null
cast send $USDT "approve(address,uint256)" $AQUA 1000000000 --private-key $BOB_KEY --rpc-url $WC >/dev/null
STRATEGY=$(cast abi-encode "f(address,address,address,uint128,uint16,bytes32)" $BOB $USDT $FILLER 2000000 30 $SALT)
cast send $AQUA "ship(address,bytes,address[],uint256[])" $POOL $STRATEGY "[$USDT]" "[2000000]" --private-key $BOB_KEY --rpc-url $WC >/dev/null
echo "bob strategy shipped; bob USDT=$(cast call $USDT 'balanceOf(address)(uint256)' $BOB --rpc-url $WC)"

# Base: give the customer 3 USDC and 3 EURC.
cast rpc anvil_impersonateAccount $DEPLOYER --rpc-url $BASE >/dev/null
for token in $USDC $EURC; do
  cast send $token "transfer(address,uint256)" $CUSTOMER 3000000 --from $DEPLOYER --unlocked --rpc-url $BASE >/dev/null
done
echo "customer USDC=$(cast call $USDC 'balanceOf(address)(uint256)' $CUSTOMER --rpc-url $BASE) EURC=$(cast call $EURC 'balanceOf(address)(uint256)' $CUSTOMER --rpc-url $BASE)"
