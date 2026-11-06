#!/usr/bin/env bash
# Stops what start.sh started (by process id) and the forks.
cd "$(dirname "$0")"
for name in order-desk relayer matchmaker; do
  [ -f ".state/$name.pid" ] && kill "$(cat ".state/$name.pid")" 2>/dev/null || true
  rm -f ".state/$name.pid"
done
pkill -f "anvil --port 860" 2>/dev/null || true
pkill -f mock_order_desk.py 2>/dev/null || true
echo stopped
