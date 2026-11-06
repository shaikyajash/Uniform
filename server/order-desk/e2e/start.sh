#!/usr/bin/env bash
# Starts the forks (A's setup), then our Order Desk and Relayer and A's Matchmaker against them.
set -euo pipefail
cd "$(dirname "$0")"
HERE=$PWD
SERVER=$(cd ../.. && pwd)
STATE=$HERE/.state
mkdir -p "$STATE"
export PATH="$HOME/.foundry/bin:$PATH"

"$SERVER/matchmaker/e2e/setup.sh"
pkill -f mock_order_desk.py 2>/dev/null || true   # our Order Desk replaces A's stand-in

BASE=http://127.0.0.1:8601
WC=http://127.0.0.1:8602
BOB=0x70997970C51812dc3A010C7d01b50e0d17dc79C8
SHOP=0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65
DB=postgres://$(whoami)@localhost/uniform_e2e
PORT=4100

# A fresh Relayer key, used on the forks only, with 1 coin of gas on each.
[ -f "$STATE/relayer.key" ] || { cast wallet new --json | python3 -c "import json,sys; d=json.load(sys.stdin); d=d.get('data',d) if isinstance(d,dict) else d; d=d[0] if isinstance(d,list) else d; print(d['private_key'])" > "$STATE/relayer.key"; chmod 600 "$STATE/relayer.key"; }
RELAYER=$(cast wallet address "$(cat "$STATE/relayer.key")")
for url in $BASE $WC; do cast rpc anvil_setBalance "$RELAYER" 0xDE0B6B3A7640000 --rpc-url $url >/dev/null; done
[ -f "$STATE/token" ] || openssl rand -hex 32 > "$STATE/token"

dropdb --if-exists uniform_e2e; createdb uniform_e2e
cd "$SERVER"
DATABASE_URL=$DB ./target/debug/order-desk add-shop --name "Kettle & Co" --payout $SHOP \
  | awk '/API key:/ {print $3}' > "$STATE/shop.key"

# Only our own earlier run: a dev Order Desk or Relayer started elsewhere keeps running.
for name in order-desk relayer matchmaker; do
  [ -f "$STATE/$name.pid" ] && kill "$(cat "$STATE/$name.pid")" 2>/dev/null || true
done
rm -f "$STATE/matchmaker.db"
sleep 1

DATABASE_URL=$DB PORT=$PORT INTERNAL_EVENTS_TOKEN=$(cat "$STATE/token") PROVIDER_ADDRESS=$BOB \
  nohup ./target/debug/order-desk serve > "$STATE/order-desk.log" 2>&1 &
echo $! > "$STATE/order-desk.pid"
DATABASE_URL=$DB RELAYER_PRIVATE_KEY=$(cat "$STATE/relayer.key") RPC_84532=$BASE RPC_1874=$WC \
  nohup ./target/debug/relayer > "$STATE/relayer.log" 2>&1 &
echo $! > "$STATE/relayer.pid"
CHAINS_PATH=../shared/chains.json ORDER_DESK_URL=http://127.0.0.1:$PORT INTERNAL_EVENTS_TOKEN=$(cat "$STATE/token") \
  FILLER_PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 BOB_ADDRESS=$BOB \
  STRATEGY_SALT=0x00000000000000000000000000000000000000000000000000000000000e2e01 STRATEGY_MAX_PER_ORDER=2000000 \
  RPC_84532=$BASE RPC_1874=$WC DB_PATH="$STATE/matchmaker.db" POLL_SECONDS=2 \
  nohup ./target/debug/matchmaker > "$STATE/matchmaker.log" 2>&1 &
echo $! > "$STATE/matchmaker.pid"
sleep 3
echo "relayer $RELAYER; order desk on :$PORT; logs in $STATE"
