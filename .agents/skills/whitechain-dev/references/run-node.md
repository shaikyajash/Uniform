# Run a Whitechain node

Deploy an external Whitechain RPC node that follows the canonical L2 chain over L1 derivation and P2P. Use this when a team needs its own RPC endpoint, an indexer or explorer backend, or a private node instead of the public RPC. Testnet only for now; mainnet ships at launch.

Source: [`whitechain-labs/node`](https://github.com/whitechain-labs/node), a standalone Docker Compose stack.

## Client

Each node is a pair of services:

- `op-reth` – execution client (a Reth fork), exposes JSON-RPC and WebSocket
- `op-node` – consensus client (OP Stack), derives the chain from L1 and peers over libp2p

The stack ships three profiles, picked with `PROFILE=<profile>` or a per-profile `make` target:

| Profile | Sync method | Storage | Use it for |
| --- | --- | --- | --- |
| `full-snap-node` | execution-layer (snap) | Pruned | Recommended default. Snap-syncs from a trusted reth peer (`WHITECHAIN_RETH_TRUSTED_PEERS`), no snapshot needed, fastest to bootstrap |
| `full-node` | consensus-layer | Pruned | No trusted peer to snap-sync from. Re-executes the chain from L1, restore a snapshot to skip the long initial sync |
| `archive-node` | consensus-layer | Full history | Explorers, indexers, historical tracing (deeper archive-node config is out of scope here) |

## Hardware requirements

| Component | `full-snap-node` / `full-node` | `archive-node` |
| --- | --- | --- |
| CPU | 4+ cores | 8+ cores |
| RAM | 16 GB | 32 GB |
| Storage | NVMe SSD, 500 GB min, 1 TB recommended | NVMe SSD, 1 TB+ |
| Network | 100 Mbps+ | 1 Gbps |

## Steps

1. Get the `public-rpc-node` manifests, place `genesis.json` and `rollup.json` under `artifacts/testnet/`.
2. `cp .env.testnet.example .env`, then fill `PUBLIC_IP`, `WHITECHAIN_PUBLIC_RPC`, `L1_RPC_URL`, `L1_BEACON_URL`, and (for `full-snap-node`) `WHITECHAIN_RETH_TRUSTED_PEERS`.
3. `L1_RPC_URL`/`L1_BEACON_URL` need your own Ethereum Sepolia RPC and Beacon API endpoints. `op-node` reads L1 batches through the RPC and blob data (EIP-4844) through the Beacon API, so a regular execution-only endpoint is not enough. Confirm both respond before filling in `.env`.
4. Start the profile: `make up-full-snap-node` (or `up-full-node` / `up-archive-node`). This validates `.env` and the artifacts, generates `keys/jwt.txt` if missing, and runs `docker compose --profile <profile> up -d`.
5. Confirm the node responds:

```bash
curl -s -X POST http://127.0.0.1:8545 \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_getBlockByNumber","params":["latest",false],"id":1}'
```

## Network ports

| Port | Default | Published on |
| --- | --- | --- |
| HTTP RPC | `8545` | All profiles |
| WebSocket RPC | `8546` | All profiles |
| op-node RPC | `9545` | All profiles, loopback (`127.0.0.1`) only |
| op-node P2P (TCP+UDP) | `9222` | All profiles |
| EL P2P (TCP+UDP) | `30303` | `full-snap-node` only |

The Engine API (`8551`) stays on the internal Compose network and is never published.

## Security

- `op-node` RPC (`9545`) is bound to loopback only on every profile, reachable from the host, never from the network. It exposes only `optimism`, `superroot`, and `opp2p`; the `admin` namespace is not enabled.
- `op-reth`'s HTTP (`8545`) and WebSocket (`8546`) are the intended public surface of this stack, since it is a public RPC node by design, not something to hide behind loopback. They expose only read-only namespaces (`eth`, `net`, `web3`, `rpc`; `archive-node` adds `debug`, `trace`, `txpool`, `reth`). Put them behind a firewall, reverse proxy, or rate limiter before serving untrusted clients, but do not block them entirely, since that defeats the point of running a public RPC node.
- The Engine API (`8551`) stays internal to the Compose network and must never be published.
- The stack holds no project-side private keys. It only follows the chain and forwards transactions to the sequencer. `keys/jwt.txt` is generated locally and used only between `op-node` and `op-reth`.

## Verify sync

Watch `make logs-<profile>`. `op-node` should show `Connected to L1 Beacon API` and `started p2p host` with a peerID early on, then:

- `full-snap-node` (snap): `Starting EL sync`, then repeating `Inserting unsafe L2 execution payload to drive EL sync` / `Inserted new L2 unsafe block` lines
- `full-node` / `archive-node` (consensus-layer): repeating `Advancing bq origin` lines (L1 batch derivation), then `Inserted new L2 unsafe block` lines

Initial sync typically completes in about an hour on testnet for `full-snap-node`. For `full-node`/`archive-node`, it ranges from minutes on a fresh testnet to many hours on a long-running chain unless a snapshot is restored.

Independently of what `op-node` reports, `op-reth` runs its own 14-stage sync pipeline (Headers, Bodies, Execution, ...). The RPC only reflects a block once the pipeline commits it, so `eth_blockNumber` can lag behind what `op-node` has already accepted. Watch pipeline progress directly:

```bash
docker logs -f whitechain-<profile>-op-reth 2>&1 | grep --line-buffered -iE "stage=|received headers|finished stage"
```

Check how far the node is behind the wall clock:

```bash
curl -s -X POST http://127.0.0.1:9545 \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"optimism_syncStatus","params":[],"id":1}'
```

Check connected peers:

```bash
curl -s -X POST http://127.0.0.1:9545 \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"opp2p_peers","params":[true],"id":1}'
```

## Error handling

| Error | Cause | Action |
| --- | --- | --- |
| `Missing .env` | No `.env` file | `cp .env.testnet.example .env` and fill the required values |
| `Missing artifacts/<network>/genesis.json` | Artifacts missing, or folder name doesn't match `WHITECHAIN_NETWORK` | Place `genesis.json`/`rollup.json` under `artifacts/<network>/` |
| Compose asks for `PUBLIC_IP` or `WHITECHAIN_PUBLIC_RPC` | Required variable unset | Fill it in `.env` |
| `WHITECHAIN_RETH_TRUSTED_PEERS` missing (`full-snap-node` only) | Snap sync needs a trusted reth enode | Set it in `.env`, form `enode://<pubkey>@<ip>:30303` |
| Sync is slow | No snapshot restored on `full-node`/`archive-node`, or a slow L1 endpoint | Restore a snapshot, use `full-snap-node` instead, or switch L1 provider |
| `optimism_syncStatus` shows `unsafe_l2.number = 0` | Engine not yet connected | Check `op-node` logs for `Inserted new L2 unsafe block`; confirm the genesis hash matches `rollup.json` |
| `nonce has already been used` when deploying | Node not fully synced | Wait for the `optimism_syncStatus` lag to drop near zero before sending transactions |
