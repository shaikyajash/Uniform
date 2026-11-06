#!/usr/bin/env bash
# Runs the Matchmaker against the local forks with anvil test keys. Build first: cargo build -p matchmaker
cd "$(dirname "$0")"
export CHAINS_PATH=../../../shared/chains.json
export ORDER_DESK_URL=http://127.0.0.1:8603
export INTERNAL_EVENTS_TOKEN=test-token
export FILLER_PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
export STRATEGY_MAKER=0x70997970C51812dc3A010C7d01b50e0d17dc79C8
export STRATEGY_SALT=0x00000000000000000000000000000000000000000000000000000000000e2e01
export STRATEGY_MAX_PER_ORDER=2000000
export RPC_84532=http://127.0.0.1:8601
export RPC_1874=http://127.0.0.1:8602
export RPC_11155111=http://127.0.0.1:8604
export DB_PATH=$PWD/.state/matchmaker.db
export POLL_SECONDS=2
exec ../../target/debug/matchmaker
