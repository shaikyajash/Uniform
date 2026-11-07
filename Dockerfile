# The Uniform backend in one image: Order Desk (HTTP API), Relayer (gas payer), Matchmaker and
# Notifier, started together by server/deploy/start.sh. Built from the repo root because the
# Rust bindings and every service read shared/ (contract ABIs, chains.json).

FROM rust:1.97-bookworm AS build
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY shared shared
COPY server server
WORKDIR /app/server
# Fewer parallel jobs keeps the compiler inside a small builder's memory.
ENV CARGO_BUILD_JOBS=2
RUN cargo build --release -p order-desk -p relayer -p matchmaker -p notifier

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 curl && rm -rf /var/lib/apt/lists/*
WORKDIR /app/server
COPY shared /app/shared
COPY --from=build /app/server/target/release/order-desk /app/server/target/release/relayer /app/server/target/release/matchmaker /app/server/target/release/notifier /usr/local/bin/
COPY server/deploy/start.sh /usr/local/bin/start.sh
# Every service defaults to ../shared/chains.json, which from /app/server is /app/shared/chains.json.
ENV CHAINS_JSON=/app/shared/chains.json CHAINS_PATH=/app/shared/chains.json
CMD ["/usr/local/bin/start.sh"]
