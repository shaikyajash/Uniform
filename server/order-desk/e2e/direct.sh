#!/usr/bin/env bash
# direct.sh <USDT amount> <label>: the shop side, end to end, on the forks.
# 1. The shop signs in to the dashboard API with its payout wallet and sets its webhook URL.
# 2. A customer pays in USDT on Whitechain (Path A): quote, sign, the Relayer sends it.
# 3. The shop refunds 0.4 USDT, then the rest, each signed by the payout wallet.
# Along the way it checks the order's status, the balances, and that the stand-in shop server got
# order.paid and order.refunded with valid signatures.
set -euo pipefail
cd "$(dirname "$0")"
STATE=$PWD/.state
export PATH="$HOME/.foundry/bin:$PATH"
DESK=http://127.0.0.1:4100
WC=http://127.0.0.1:8602
USDT=0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7
DEPLOYER=0x7801aE8881A59F231fA31D7a5b56cb20766fd2eB
# anvil's public test wallets: #4 is the shop's payout wallet (start.sh), #5 a Path A customer.
MNEMONIC="test test test test test test test test test test test junk"
SHOP_KEY=$(cast wallet private-key --mnemonic "$MNEMONIC" --mnemonic-index 4)
CUSTOMER_KEY=$(cast wallet private-key --mnemonic "$MNEMONIC" --mnemonic-index 5)
SHOP=$(cast wallet address "$SHOP_KEY")
CUSTOMER=$(cast wallet address "$CUSTOMER_KEY")
FIRST_REFUND=0.4

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1], {'d': d}))" "$1"; }
usdt() { cast call $USDT "balanceOf(address)(uint256)" "$1" --rpc-url $WC | awk '{print $1}'; }
status() { curl -s "$DESK/v1/orders/$ORDER" | json "d['status']"; }
wait_for() {
  for _ in $(seq 1 ${2:-90}); do [ "$(status)" = "$1" ] && return 0; sleep 1; done
  echo "timed out waiting for $1; order is $(status)"; curl -s "$DESK/v1/orders/$ORDER"; echo; exit 1
}
# Waits until the stand-in shop server has logged <event> for this order with a valid signature.
wait_webhook() {
  for _ in $(seq 1 30); do
    python3 - "$STATE/webhooks.jsonl" "$1" "$ORDER" <<'EOF' && { echo "webhook $1: received, signature valid"; return 0; }
import json, sys
path, event, order = sys.argv[1:]
try:
    lines = [json.loads(line) for line in open(path)]
except FileNotFoundError:
    sys.exit(1)
sys.exit(0 if any(l["event"] == event and l["valid"] and l["body"]["orderId"].lower() == order.lower() for l in lines) else 1)
EOF
    sleep 1
  done
  echo "no valid $1 webhook for $ORDER"; cat "$STATE/webhooks.jsonl" 2>/dev/null; exit 1
}
shop_call() { curl -s -H "Authorization: Bearer $SESSION" -H 'content-type: application/json' "$@"; }

# 1. Dashboard sign-in: sign the challenge message with the payout wallet.
curl -s -X POST $DESK/v1/auth/challenge -H 'content-type: application/json' -d "{\"address\":\"$SHOP\"}" > "$STATE/challenge.json"
MESSAGE=$(json "d['message']" < "$STATE/challenge.json")
NONCE=$(json "d['nonce']" < "$STATE/challenge.json")
SIGNATURE=$(cast wallet sign --private-key "$SHOP_KEY" "$MESSAGE")
SESSION=$(curl -s -X POST $DESK/v1/auth/verify -H 'content-type: application/json' \
  -d "{\"nonce\":\"$NONCE\",\"signature\":\"$SIGNATURE\"}" | json "d['token']")
echo "signed in as $(shop_call $DESK/v1/shop | json "d['name']")"
shop_call -X PATCH $DESK/v1/shop -d '{"webhookUrl":"http://127.0.0.1:4199/uniform"}' > "$STATE/shop.json"
json "d['webhookSecret']" < "$STATE/shop.json" > "$STATE/webhook.secret"
echo "webhook URL set: $(json "d['webhookUrl']" < "$STATE/shop.json")"

# 2. A Path A payment. The customer needs USDT only: the Relayer pays the gas.
cast rpc anvil_impersonateAccount $DEPLOYER --rpc-url $WC >/dev/null
cast send $USDT "transfer(address,uint256)" "$CUSTOMER" 5000000 --from $DEPLOYER --unlocked --rpc-url $WC >/dev/null
shop_before=$(usdt "$SHOP"); customer_before=$(usdt "$CUSTOMER")

ORDER=$(curl -s -X POST $DESK/v1/orders -H "Authorization: Bearer $(cat "$STATE/shop.key")" -H 'content-type: application/json' \
  -d "{\"amount\":\"$1\",\"reference\":\"e2e-$2\"}" | json "d['orderId']")
echo "order $ORDER ($1 USDT)"
curl -s -X POST "$DESK/v1/orders/$ORDER/quote" -H 'content-type: application/json' -d "{\"payer\":\"$CUSTOMER\"}" > "$STATE/quote.json"
echo "quote: path $(json "d['path']" < "$STATE/quote.json"), total $(json "d['total']" < "$STATE/quote.json") USDT"
json "__import__('json').dumps(d['toSign'][0]['typedData'])" < "$STATE/quote.json" > "$STATE/typed.json"
SIGNATURE=$(cast wallet sign --private-key "$CUSTOMER_KEY" --data --from-file "$STATE/typed.json")
curl -s -X POST "$DESK/v1/orders/$ORDER/direct" -H 'content-type: application/json' \
  -d "{\"quoteId\":\"$(json "d['quoteId']" < "$STATE/quote.json")\",\"signature\":\"$SIGNATURE\"}"; echo
wait_for paid
echo "paid: shop USDT $shop_before -> $(usdt "$SHOP"); customer USDT $customer_before -> $(usdt "$CUSTOMER")"
wait_webhook order.paid

# 3. Refunds: the Order Desk writes the note, the payout wallet signs it, the Relayer sends it.
refund() {
  shop_call -X POST "$DESK/v1/orders/$ORDER/refund-note" -d "{\"amount\":\"$1\"}" > "$STATE/refund-note.json"
  json "__import__('json').dumps(d['typedData'])" < "$STATE/refund-note.json" > "$STATE/refund-typed.json"
  local signature
  signature=$(cast wallet sign --private-key "$SHOP_KEY" --data --from-file "$STATE/refund-typed.json")
  shop_call -X POST "$DESK/v1/orders/$ORDER/refunds" -d "$(python3 - "$STATE/refund-note.json" "$signature" <<'EOF'
import json, sys
note = json.load(open(sys.argv[1]))
print(json.dumps({"amount": note["amount"], "validAfter": note["validAfter"], "validBefore": note["validBefore"], "signature": sys.argv[2]}))
EOF
)"; echo
}
rest=$(python3 -c "from decimal import Decimal as D; print(D('$1') - D('$FIRST_REFUND'))")
refund "$FIRST_REFUND"
wait_for partially_refunded
echo "refunded $FIRST_REFUND: customer USDT now $(usdt "$CUSTOMER")"
refund "$rest"
wait_for refunded
echo "refunded the rest ($rest): shop USDT $(usdt "$SHOP") (was $shop_before); customer USDT $(usdt "$CUSTOMER") (was $customer_before)"
wait_webhook order.refunded
echo "dashboard list: $(shop_call "$DESK/v1/orders?status=refunded" | json "[(o['reference'], o['status'], o['refunded']) for o in d['orders']]")"
