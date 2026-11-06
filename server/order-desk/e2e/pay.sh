#!/usr/bin/env bash
# pay.sh <USDT amount> <secret label> [nofill|split|split-missing]: one cross-chain payment
# through the real API.
# Default: one lock on Base. Waits for Bob's fill, reveals the secret, waits for paid and Bob's claim.
# nofill: the provider withdraws Aqua's approval right after the quote, so nobody fills; waits for
#   the lock, moves the clocks past the lock's expiry, and waits for the Relayer's refund.
# split: 1 USDC on Base and the rest on Ethereum Sepolia; then as the default, with both claims.
# split-missing: as split, but the customer has no USDC on Ethereum, so that lock never opens.
#   Bob must not fill; the Base lock is refunded after expiry.
set -euo pipefail
cd "$(dirname "$0")"
STATE=$PWD/.state
export PATH="$HOME/.foundry/bin:$PATH"
DESK=http://127.0.0.1:4100
BASE=http://127.0.0.1:8601
WC=http://127.0.0.1:8602
ETH=http://127.0.0.1:8604
ETH_USDC=0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238
USDC=0x036CbD53842c5426634e7929541eC2318f3dCF7e
USDT=0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7
SHOP=0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65
PROVIDER=0x70997970C51812dc3A010C7d01b50e0d17dc79C8
AQUA=0x02dF2659B5B4956EAFEe3293584C2aaefC114871
CUSTOMER_KEY=$(cat ../../matchmaker/e2e/.state/customer.key)
CUSTOMER=$(cast wallet address "$CUSTOMER_KEY")
SECRET=$(cast keccak "$2")
HASHLOCK=$(cast keccak "$SECRET")
json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1], {'d': d}))" "$1"; }
status() { curl -s "$DESK/v1/orders/$ORDER" | json "d['status']"; }
expect() { [ "$1" = "$2" ] || { echo "FAIL: $3: got $1, wanted $2"; exit 1; }; }
wait_for() {
  for _ in $(seq 1 ${2:-120}); do [ "$(status)" = "$1" ] && return 0; sleep 1; done
  echo "timed out waiting for $1; order is $(status)"; curl -s "$DESK/v1/orders/$ORDER"; echo; exit 1
}

shop_before=$(cast call $USDT "balanceOf(address)(uint256)" $SHOP --rpc-url $WC | awk '{print $1}')
customer_before=$(cast call $USDC "balanceOf(address)(uint256)" $CUSTOMER --rpc-url $BASE | awk '{print $1}')

ORDER=$(curl -s -X POST $DESK/v1/orders -H "Authorization: Bearer $(cat "$STATE/shop.key")" -H 'content-type: application/json' \
  -d "{\"amount\":\"$1\",\"reference\":\"e2e-$2\"}" | json "d['orderId']")
echo "order $ORDER ($1 USDT)"
MODE=${3:-}
case $MODE in
  split|split-missing) PLAN='"legs":[{"chainId":84532,"amount":"1"},{"chainId":11155111}]' ;;
  *) PLAN='"chainId":84532' ;;
esac
if [ "$MODE" = split-missing ]; then
  # Empty the customer's Ethereum USDC, so the Relayer refuses that lock.
  cast rpc anvil_setBalance "$CUSTOMER" 0xDE0B6B3A7640000 --rpc-url $ETH >/dev/null
  held=$(cast call $ETH_USDC "balanceOf(address)(uint256)" $CUSTOMER --rpc-url $ETH | awk '{print $1}')
  cast send $ETH_USDC "transfer(address,uint256)" 0x000000000000000000000000000000000000dEaD "$held" \
    --private-key "$CUSTOMER_KEY" --rpc-url $ETH >/dev/null
fi
curl -s -X POST "$DESK/v1/orders/$ORDER/quote" -H 'content-type: application/json' \
  -d "{\"payer\":\"$CUSTOMER\",\"hashlock\":\"$HASHLOCK\",$PLAN}" > "$STATE/quote.json"
QUOTE=$(json "d['quoteId']" < "$STATE/quote.json")
provider_approves() {
  cast rpc anvil_impersonateAccount $PROVIDER --rpc-url $WC >/dev/null
  cast send $USDT "approve(address,uint256)" $AQUA "$1" --from $PROVIDER --unlocked --rpc-url $WC >/dev/null
}
if [ "$MODE" = nofill ]; then
  approved=$(cast call $USDT "allowance(address,address)(uint256)" $PROVIDER $AQUA --rpc-url $WC | awk '{print $1}')
  provider_approves 0
fi
NOTES=$(json "len(d['toSign'])" < "$STATE/quote.json")
echo "quote: $(json "d['total']" < "$STATE/quote.json") USDC in $NOTES lock(s): $(json "' + '.join(n['amount'] + ' on ' + str(n['chainId']) for n in d['toSign'])" < "$STATE/quote.json")"
SIGNATURES=""
for i in $(seq 0 $((NOTES - 1))); do
  json "__import__('json').dumps(d['toSign'][$i]['typedData'])" < "$STATE/quote.json" > "$STATE/typed.json"
  SIGNATURES="$SIGNATURES${SIGNATURES:+,}\"$(cast wallet sign --private-key "$CUSTOMER_KEY" --data --from-file "$STATE/typed.json")\""
done
BODY=$(printf '{"quoteId":"%s","signatures":[%s]}' "$QUOTE" "$SIGNATURES")
curl -s -X POST "$DESK/v1/orders/$ORDER/legs" -H 'content-type: application/json' -d "$BODY"; echo
leg_status() { curl -s "$DESK/v1/orders/$ORDER" | json "d['legs'][$1]['status']"; }

if [ "$MODE" = split-missing ]; then
  for _ in $(seq 1 120); do [ "$(leg_status 0)" = locked ] && break; sleep 1; done
  echo "Base lock: $(leg_status 0); Ethereum lock job: $(psql -tAq uniform_e2e -c "SELECT status || ' ' || coalesce(last_error, '') FROM relayer_jobs WHERE order_id = '$ORDER' AND chain_id = 11155111 ORDER BY id DESC LIMIT 1")"
  sleep 20
  echo "after 20 s: order $(status), destination lock $(curl -s "$DESK/v1/orders/$ORDER" | json "d['destinationLockId']") (Bob must not fill)"
  for url in $BASE $WC $ETH; do cast rpc evm_increaseTime 1500 --rpc-url $url >/dev/null; cast rpc evm_mine --rpc-url $url >/dev/null; done
  psql -q uniform_e2e -c "UPDATE legs SET expiry = expiry - 1500 WHERE order_id = '$ORDER'"
  for _ in $(seq 1 180); do [ "$(leg_status 0)" = returned ] && break; sleep 1; done
  customer_after=$(cast call $USDC "balanceOf(address)(uint256)" $CUSTOMER --rpc-url $BASE | awk '{print $1}')
  echo "Base lock $(leg_status 0); order $(status); customer Base USDC $customer_before -> $customer_after"
  # Give the customer's Ethereum USDC back, so a later split run on these forks works.
  cast rpc anvil_impersonateAccount 0x000000000000000000000000000000000000dEaD --rpc-url $ETH >/dev/null
  cast rpc anvil_setBalance 0x000000000000000000000000000000000000dEaD 0xDE0B6B3A7640000 --rpc-url $ETH >/dev/null
  cast send $ETH_USDC "transfer(address,uint256)" $CUSTOMER "$held" --from 0x000000000000000000000000000000000000dEaD --unlocked --rpc-url $ETH >/dev/null
  expect "$(curl -s "$DESK/v1/orders/$ORDER" | json "d['destinationLockId']")" None "Bob must not fill"
  expect "$(leg_status 0)" returned "the Base lock is refunded"
  expect "$(status)" returned "the order ends returned"
  expect "$customer_after" "$customer_before" "the customer has all their Base USDC back"
  echo PASS
  exit 0
fi

if [ "$MODE" = nofill ]; then
  wait_for source_locked 300
  echo "locked; Bob will not fill. Moving the clocks 25 minutes on."
  for url in $BASE $WC $ETH; do cast rpc evm_increaseTime 1500 --rpc-url $url >/dev/null; cast rpc evm_mine --rpc-url $url >/dev/null; done
  # The Order Desk's sweeper reads the wall clock: let it see the lock as expired too.
  psql -q uniform_e2e -c "UPDATE legs SET expiry = expiry - 1500 WHERE order_id = '$ORDER'"
  wait_for returned 180
  customer_after=$(cast call $USDC "balanceOf(address)(uint256)" $CUSTOMER --rpc-url $BASE | awk '{print $1}')
  echo "returned: customer USDC $customer_before -> $customer_after"
  provider_approves "$approved"
  expect "$customer_after" "$customer_before" "the customer has all their USDC back"
  echo PASS
  exit 0
fi

wait_for destination_locked 300
echo "Bob filled; revealing the secret"
curl -s -X POST "$DESK/v1/orders/$ORDER/secret" -H 'content-type: application/json' -d "{\"secret\":\"$SECRET\"}"; echo
wait_for paid 90
shop_after=$(cast call $USDT "balanceOf(address)(uint256)" $SHOP --rpc-url $WC | awk '{print $1}')
customer_after=$(cast call $USDC "balanceOf(address)(uint256)" $CUSTOMER --rpc-url $BASE | awk '{print $1}')
echo "paid: shop USDT $shop_before -> $shop_after; customer USDC $customer_before -> $customer_after"
for i in $(seq 0 $((NOTES - 1))); do
  for _ in $(seq 1 90); do [ "$(leg_status $i)" = claimed ] && break; sleep 1; done
  echo "Bob's source claim $((i + 1)) of $NOTES: leg is $(leg_status $i)"
  expect "$(leg_status $i)" claimed "Bob redeems lock $((i + 1))"
done
expect "$((shop_after - shop_before))" "$(python3 -c "print(round($1 * 10**6))")" "the shop gets the order amount"
echo PASS
