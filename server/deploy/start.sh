#!/usr/bin/env bash
# Runs the four backend services in one container. If any one stops, the container stops, so the
# host restarts all of them instead of serving a healthy API over a dead Matchmaker or Relayer.
set -euo pipefail

: "${DATABASE_URL:?DATABASE_URL is required}"
: "${INTERNAL_EVENTS_TOKEN:?INTERNAL_EVENTS_TOKEN is required}"
: "${RELAYER_PRIVATE_KEY:?RELAYER_PRIVATE_KEY is required}"
: "${FILLER_PRIVATE_KEY:?FILLER_PRIVATE_KEY is required}"

export PORT="${PORT:-10000}"
# The Matchmaker reaches the Order Desk inside the container.
export ORDER_DESK_URL="http://127.0.0.1:${PORT}"
export DB_PATH="${DB_PATH:-/app/data/matchmaker.db}"
mkdir -p "$(dirname "$DB_PATH")"

order-desk serve &
# Give the Order Desk a moment to run its migrations before the others read the tables.
for _ in $(seq 1 30); do
  curl -fsS "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1 && break
  sleep 1
done
relayer &
matchmaker &
notifier &

wait -n
echo "start.sh: a service stopped; stopping the container so it restarts" >&2
exit 1
