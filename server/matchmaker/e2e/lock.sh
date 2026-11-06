#!/usr/bin/env bash
# lock.sh <order-label> <secret-label>: add a 1 USDT order to the mock Order Desk, then the customer locks for Bob.
# Locks 1.003 USDC by default; TOKEN=<address> AMOUNT=<units> locks another token (for EURC, about 0.905 EURC covers it).
set -e; cd "$(dirname "$0")"
ORDER=$(cast keccak "$1"); SECRET=$(cast keccak "$2")
python3 - "$ORDER" <<'PY'
import json,sys; p='.state/orders.json'
try: d=json.load(open(p))
except Exception: d={}
d[sys.argv[1]]={"shop":{"name":"Kettle & Co","address":"0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65"},"amount":"1.000000","status":"quoted"}
json.dump(d,open(p,'w'))
PY
HERE=$PWD; cd ../../../contracts
SOURCE_LOCK=0x02dF2659B5B4956EAFEe3293584C2aaefC114871 CUSTOMER_PRIVATE_KEY=$(cat $HERE/.state/customer.key) GAS_PAYER_PRIVATE_KEY=0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6 BOB_ADDRESS=0x70997970C51812dc3A010C7d01b50e0d17dc79C8 ORDER_ID=$ORDER SECRET=$SECRET TOKEN=${TOKEN:-0x036CbD53842c5426634e7929541eC2318f3dCF7e} AMOUNT=${AMOUNT:-1003000} MIN_USDC_OUT=${MIN_USDC_OUT:-0} \
  forge script script/LockForBob.s.sol --rpc-url http://127.0.0.1:8601 --broadcast 2>&1 | grep -A1 "source lockId" | tail -1
