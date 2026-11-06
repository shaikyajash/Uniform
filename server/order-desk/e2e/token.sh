#!/usr/bin/env bash
# token.sh <USDT amount> <secret label>: one payment in WBTC from Ethereum Sepolia, gas-free
# through the customer's Calibur account (EIP-7702), on the local forks (start.sh running).
#
# Puts this branch's SourceLock (any ERC-20) at the fork's SourceLock address, gives a fresh
# customer WBTC from Aave's test faucet and no ETH, then signs what the pay page signs: the 7702
# authorization to Calibur and the Calibur batch (approve + lock). Checks that the shop is paid,
# Bob redeems the WBTC, the customer's wallet is delegated to Calibur, and it spent no ETH.
set -euo pipefail
cd "$(dirname "$0")"
STATE=$PWD/.state
export PATH="$HOME/.foundry/bin:$PATH"
DESK=http://127.0.0.1:4100
WC=http://127.0.0.1:8602
ETH=http://127.0.0.1:8604
CHAINS=../../../shared/chains.json
USDT=0xcedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7
SHOP=0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65
BOB=0x70997970C51812dc3A010C7d01b50e0d17dc79C8
FAUCET=0xC959483DBa39aa9E78757139af0e9a2EDEb3f42D
# anvil's first test key: only pays gas for setup on the fork.
SETUP_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
pick() { python3 -c "import json; c=json.load(open('$CHAINS'))['ethereumSepolia']; print($1)"; }
LOCK=$(pick "c['contracts']['sourceLock']")
CALIBUR=$(pick "c['contracts']['calibur']")
WBTC=$(pick "c['tokens']['WBTC']['address']")
ETH_USDC=$(pick "c['tokens']['USDC']['address']")
ETH_EURC=$(pick "c['tokens']['EURC']['address']")
SECRET=$(cast keccak "$2")
HASHLOCK=$(cast keccak "$SECRET")
json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1], {'d': d}))" "$1"; }
status() { curl -s "$DESK/v1/orders/$ORDER" | json "d['status']"; }
leg_status() { curl -s "$DESK/v1/orders/$ORDER" | json "d['legs'][0]['status']"; }
expect() { [ "$1" = "$2" ] || { echo "FAIL: $3: got $1, wanted $2"; exit 1; }; }
wait_for() {
  for _ in $(seq 1 ${2:-120}); do [ "$(status)" = "$1" ] && return 0; sleep 1; done
  echo "timed out waiting for $1; order is $(status)"; curl -s "$DESK/v1/orders/$ORDER"; echo; exit 1
}
balance() { cast call "$1" "balanceOf(address)(uint256)" "$2" --rpc-url "$3" | awk '{print $1}'; }

# This branch's SourceLock, deployed on the fork and its code put at the chains.json address.
cast rpc anvil_setBalance "$(cast wallet address $SETUP_KEY)" 0xDE0B6B3A7640000 --rpc-url $ETH >/dev/null
# (--constructor-args takes every argument after it, so it goes last.)
NEW=$(cd ../../../contracts && forge create src/SourceLock.sol:SourceLock --broadcast --json \
  --private-key $SETUP_KEY --rpc-url $ETH --constructor-args "$ETH_USDC" "$ETH_EURC" | json "d['deployedTo']")
cast rpc anvil_setCode "$LOCK" "$(cast code "$NEW" --rpc-url $ETH)" --rpc-url $ETH >/dev/null
echo "SourceLock at $LOCK now runs this branch's code"

# A fresh customer: WBTC from Aave's faucet, and no ETH at all.
CUSTOMER_KEY=$(cast wallet new --json | json "(d[0] if isinstance(d, list) else d)['private_key']")
CUSTOMER=$(cast wallet address "$CUSTOMER_KEY")
cast send $FAUCET "mint(address,address,uint256)" "$WBTC" "$CUSTOMER" 100000000 --private-key $SETUP_KEY --rpc-url $ETH >/dev/null
cast rpc anvil_setBalance "$CUSTOMER" 0x0 --rpc-url $ETH >/dev/null
wbtc_before=$(balance "$WBTC" "$CUSTOMER" $ETH)
bob_before=$(balance "$WBTC" $BOB $ETH)
shop_before=$(balance $USDT $SHOP $WC)
echo "customer $CUSTOMER: $wbtc_before WBTC units, $(cast balance "$CUSTOMER" --rpc-url $ETH) wei"

ORDER=$(curl -s -X POST $DESK/v1/orders -H "Authorization: Bearer $(cat "$STATE/shop.key")" -H 'content-type: application/json' \
  -d "{\"amount\":\"$1\",\"reference\":\"e2e-$2\"}" | json "d['orderId']")
echo "order $ORDER ($1 USDT)"
curl -s -X POST "$DESK/v1/orders/$ORDER/quote" -H 'content-type: application/json' \
  -d "{\"payer\":\"$CUSTOMER\",\"hashlock\":\"$HASHLOCK\",\"chainId\":11155111,\"token\":\"WBTC\"}" > "$STATE/quote.json"
QUOTE=$(json "d['quoteId']" < "$STATE/quote.json")
expect "$(json "d['toSign'][0]['signing']" < "$STATE/quote.json")" batch "a WBTC quote is a Calibur batch"
AMOUNT=$(json "d['toSign'][0]['amount']" < "$STATE/quote.json")
echo "quote: $AMOUNT WBTC"

# What the pay page signs: the batch, and (first payment) the 7702 authorization to Calibur.
json "__import__('json').dumps(d['toSign'][0]['typedData'])" < "$STATE/quote.json" > "$STATE/typed.json"
SIGNATURE=$(cast wallet sign --private-key "$CUSTOMER_KEY" --data --from-file "$STATE/typed.json")
AUTH_RLP=$(cast wallet sign-auth "$CALIBUR" --private-key "$CUSTOMER_KEY" --nonce "$(cast nonce "$CUSTOMER" --rpc-url $ETH)" --chain 11155111)
AUTH=$(python3 - "$AUTH_RLP" <<'EOF'
import json, sys
data = bytes.fromhex(sys.argv[1].removeprefix("0x"))
def item(b, i):
    p = b[i]
    if p < 0x80: return b[i:i+1], i + 1
    if p < 0xb8: n = p - 0x80; return b[i+1:i+1+n], i + 1 + n
    if p < 0xc0: m = p - 0xb7; n = int.from_bytes(b[i+1:i+1+m], "big"); return b[i+1+m:i+1+m+n], i + 1 + m + n
    if p < 0xf8: n = p - 0xc0; return b[i+1:i+1+n], i + 1 + n
    m = p - 0xf7; n = int.from_bytes(b[i+1:i+1+m], "big"); return b[i+1+m:i+1+m+n], i + 1 + m + n
body, _ = item(data, 0)
fields, i = [], 0
while i < len(body):
    f, i = item(body, i); fields.append(f)
num = lambda f: int.from_bytes(f, "big")
chain, address, nonce, parity, r, s = fields
print(json.dumps({"chainId": num(chain), "address": "0x" + address.hex(), "nonce": num(nonce), "yParity": num(parity), "r": hex(num(r)), "s": hex(num(s))}))
EOF
)
BODY=$(printf '{"quoteId":"%s","signatures":["%s"],"authorization":%s}' "$QUOTE" "$SIGNATURE" "$AUTH")
curl -s -X POST "$DESK/v1/orders/$ORDER/legs" -H 'content-type: application/json' -d "$BODY"; echo

wait_for destination_locked 300
code=$(cast code "$CUSTOMER" --rpc-url $ETH)
echo "Bob filled; the customer's code is $code. Revealing the secret"
curl -s -X POST "$DESK/v1/orders/$ORDER/secret" -H 'content-type: application/json' -d "{\"secret\":\"$SECRET\"}"; echo
wait_for paid 90
for _ in $(seq 1 120); do [ "$(leg_status)" = claimed ] && break; sleep 1; done
shop_after=$(balance $USDT $SHOP $WC)
wbtc_after=$(balance "$WBTC" "$CUSTOMER" $ETH)
bob_after=$(balance "$WBTC" $BOB $ETH)
eth_after=$(cast balance "$CUSTOMER" --rpc-url $ETH)
units=$(python3 -c "print(round($AMOUNT * 10**8))")
echo "paid: shop USDT +$((shop_after - shop_before)); customer WBTC -$((wbtc_before - wbtc_after)); Bob WBTC +$((bob_after - bob_before)); customer ETH $eth_after"
expect "$(echo "$code" | tr 'A-F' 'a-f')" "0xef0100$(echo "${CALIBUR#0x}" | tr 'A-F' 'a-f')" "the customer's wallet is delegated to Calibur"
expect "$((shop_after - shop_before))" "$(python3 -c "print(round($1 * 10**6))")" "the shop gets the order amount"
expect "$((wbtc_before - wbtc_after))" "$units" "the customer pays the quoted WBTC"
expect "$(leg_status)" claimed "Bob redeems the WBTC lock"
expect "$((bob_after - bob_before))" "$units" "Bob receives the WBTC"
expect "$eth_after" 0 "the customer spent no ETH"
echo PASS
