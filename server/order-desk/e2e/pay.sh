#!/usr/bin/env bash
# pay.sh <USDT amount> <secret label> [nofill]: one cross-chain payment through the real API.
# Without nofill: waits for Bob's fill, reveals the secret, waits for paid and Bob's claim.
# With nofill (an order over Bob's limit): waits for the lock, moves both clocks past the lock's
# expiry, and waits for the Relayer's refund.
set -euo pipefail
cd "$(dirname "$0")"
STATE=$PWD/.state
export PATH="$HOME/.foundry/bin:$PATH"
DESK=http://127.0.0.1:4100
BASE=http://127.0.0.1:8601
WC=http://127.0.0.1:8602
USDC=0x036CbD53842c5426634e7929541eC2318f3dCF7e
USDT=0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7
SHOP=0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65
CUSTOMER_KEY=$(cat ../../matchmaker/e2e/.state/customer.key)
CUSTOMER=$(cast wallet address "$CUSTOMER_KEY")
SECRET=$(cast keccak "$2")
HASHLOCK=$(cast keccak "$SECRET")
json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1], {'d': d}))" "$1"; }
status() { curl -s "$DESK/v1/orders/$ORDER" | json "d['status']"; }
wait_for() {
  for _ in $(seq 1 ${2:-120}); do [ "$(status)" = "$1" ] && return 0; sleep 1; done
  echo "timed out waiting for $1; order is $(status)"; curl -s "$DESK/v1/orders/$ORDER"; echo; exit 1
}

shop_before=$(cast call $USDT "balanceOf(address)(uint256)" $SHOP --rpc-url $WC | awk '{print $1}')
customer_before=$(cast call $USDC "balanceOf(address)(uint256)" $CUSTOMER --rpc-url $BASE | awk '{print $1}')

ORDER=$(curl -s -X POST $DESK/v1/orders -H "Authorization: Bearer $(cat "$STATE/shop.key")" -H 'content-type: application/json' \
  -d "{\"amount\":\"$1\",\"reference\":\"e2e-$2\"}" | json "d['orderId']")
echo "order $ORDER ($1 USDT)"
curl -s -X POST "$DESK/v1/orders/$ORDER/quote" -H 'content-type: application/json' \
  -d "{\"payer\":\"$CUSTOMER\",\"hashlock\":\"$HASHLOCK\",\"chainId\":84532}" > "$STATE/quote.json"
QUOTE=$(json "d['quoteId']" < "$STATE/quote.json")
echo "quote: $(json "d['total']" < "$STATE/quote.json") USDC, lock $(json "d['toSign'][0]['lockId']" < "$STATE/quote.json")"
json "__import__('json').dumps(d['toSign'][0]['typedData'])" < "$STATE/quote.json" > "$STATE/typed.json"
SIGNATURE=$(cast wallet sign --private-key "$CUSTOMER_KEY" --data --from-file "$STATE/typed.json")
curl -s -X POST "$DESK/v1/orders/$ORDER/legs" -H 'content-type: application/json' \
  -d "{\"quoteId\":\"$QUOTE\",\"signature\":\"$SIGNATURE\"}"; echo

if [ "${3:-}" = nofill ]; then
  wait_for source_locked 300
  echo "locked; Bob will not fill. Moving the clocks 25 minutes on."
  for url in $BASE $WC; do cast rpc evm_increaseTime 1500 --rpc-url $url >/dev/null; cast rpc evm_mine --rpc-url $url >/dev/null; done
  # The Order Desk's sweeper reads the wall clock: let it see the lock as expired too.
  psql -q uniform_e2e -c "UPDATE legs SET expiry = expiry - 1500 WHERE order_id = '$ORDER'"
  wait_for returned 180
  customer_after=$(cast call $USDC "balanceOf(address)(uint256)" $CUSTOMER --rpc-url $BASE | awk '{print $1}')
  echo "returned: customer USDC $customer_before -> $customer_after (unchanged means refunded in full)"
  exit 0
fi

wait_for destination_locked 300
echo "Bob filled; revealing the secret"
curl -s -X POST "$DESK/v1/orders/$ORDER/secret" -H 'content-type: application/json' -d "{\"secret\":\"$SECRET\"}"; echo
wait_for paid 90
shop_after=$(cast call $USDT "balanceOf(address)(uint256)" $SHOP --rpc-url $WC | awk '{print $1}')
customer_after=$(cast call $USDC "balanceOf(address)(uint256)" $CUSTOMER --rpc-url $BASE | awk '{print $1}')
echo "paid: shop USDT $shop_before -> $shop_after; customer USDC $customer_before -> $customer_after"
for _ in $(seq 1 60); do
  leg=$(curl -s "$DESK/v1/orders/$ORDER" | json "d['legs'][0]['status']")
  [ "$leg" = claimed ] && break; sleep 1
done
echo "Bob's source claim: leg is $leg"
