# Rinku 3-Node Testnet Setup Guide

This guide explains how to set up a 3-node testnet across:
1. **Replit** (https://rinkuchan.com)
2. **Local Mac** (localhost:3001)
3. **Fly.io** (https://rinku-fly.fly.dev)

## Prerequisites

- Rust installed locally (`rustup`)
- Node.js 18+ installed
- Fly.io CLI (`flyctl`) installed (optional for Fly.io deployment)

## Node Configuration

Each node uses environment variables for configuration:

| Variable | Description | Default |
|----------|-------------|---------|
| `RINKU_PORT` | HTTP API port | 3001 |
| `NODE_PEERS` | Comma-separated list of peer node URLs | "" |
| `RINKU_DB_PATH` | Database storage path | ./rinku-data |
| `RUST_LOG` | Log level (debug, info, warn, error) | info |

## Architecture

```
┌──────────────────┐     ┌──────────────────┐     ┌──────────────────┐
│   Replit Node    │◄───►│   Local Mac      │◄───►│   Fly.io Node    │
│ rinkuchan.com    │     │ localhost:3001   │     │ rinku-fly.fly.dev│
│                  │◄───────────────────────────►│                  │
└──────────────────┘     └──────────────────┘     └──────────────────┘
```

## 1. Replit Node Setup (Primary/Production)

The Replit node is already configured and deployed at https://rinkuchan.com.

**Environment variables (set in Replit Secrets):**
```bash
NODE_PEERS=https://rinku-fly.fly.dev
```

The Replit node will automatically accept connections from other nodes.

## 2. Local Mac Node Setup

### Start the local node:

```bash
# Clone the repo (if not already done)
cd /path/to/rinku

# Build and run with peer configuration
RINKU_PORT=3001 \
NODE_PEERS="https://rinkuchan.com,https://rinku-fly.fly.dev" \
cargo run -p rinku-node

# Or use the local testnet script for multiple local nodes:
./scripts/local-testnet.sh start 2
```

### Verify connectivity:
```bash
curl http://localhost:3001/api/sync/status
curl http://localhost:3001/api/peers
```

### Run the activity bot against local node:
```bash
RINKU_NODE_URL=http://localhost:3001 npm run activity-bot
```

## 3. Fly.io Node Setup (Optional)

### Create a new Fly.io app:

```bash
flyctl launch --name rinku-fly --no-deploy
```

### Create `fly.toml`:

```toml
app = "rinku-fly"
primary_region = "sjc"

[build]
  dockerfile = "Dockerfile.fly"

[env]
  RINKU_PORT = "8080"
  NODE_PEERS = "https://rinkuchan.com"
  RUST_LOG = "info"

[[services]]
  internal_port = 8080
  protocol = "tcp"
  
  [[services.ports]]
    handlers = ["http"]
    port = 80
  
  [[services.ports]]
    handlers = ["tls", "http"]
    port = 443

  [services.concurrency]
    hard_limit = 100
    soft_limit = 50
```

### Create `Dockerfile.fly`:

```dockerfile
FROM rust:1.75 AS builder
WORKDIR /app
COPY . .
RUN cargo build -p rinku-node --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/rinku-node /usr/local/bin/
EXPOSE 8080
CMD ["rinku-node"]
```

### Deploy:

```bash
flyctl deploy
```

## Validation Scripts

### Validate all nodes:

```bash
# Validate single node
npx ts-node scripts/validate-testnet.ts https://rinkuchan.com

# Validate multi-node consensus
npx ts-node scripts/validate-multi-node.ts \
  https://rinkuchan.com \
  http://localhost:3001 \
  https://rinku-fly.fly.dev
```

### Generate and verify proofs:

```bash
# Generate Profile A/B/C proofs from live data
npx ts-node scripts/generate-proofs.ts https://rinkuchan.com --count 5

# Benchmark proof sizes
npx ts-node scripts/proof-size-benchmark.ts https://rinkuchan.com --samples 20
```

### Local multi-node testing:

```bash
# Start a 3-node local testnet
./scripts/local-testnet.sh start 3

# Check status
./scripts/local-testnet.sh status

# Validate consensus
./scripts/local-testnet.sh validate

# View logs
./scripts/local-testnet.sh logs 1

# Stop all nodes
./scripts/local-testnet.sh stop
```

## Network Topology Options

### Star Topology (Recommended for testing)
All nodes connect to the primary Replit node:
```
Local ──► Replit ◄── Fly.io
```

### Mesh Topology (Full connectivity)
All nodes know about each other:
```
NODE_PEERS (Replit):   https://rinku-fly.fly.dev
NODE_PEERS (Local):    https://rinkuchan.com,https://rinku-fly.fly.dev
NODE_PEERS (Fly.io):   https://rinkuchan.com
```

## Troubleshooting

### Nodes not syncing
1. Check peer connectivity: `curl http://localhost:3001/api/peers`
2. Verify network access (firewall, NAT)
3. Check logs for connection errors

### Checkpoint height mismatch
- Normal during initial sync - wait for nodes to catch up
- Run `validate-multi-node.ts` to check sync status

### Transaction count differs
- May be due to sync lag
- Check if transactions are in mempool vs finalized

## Monitoring Commands

```bash
# Quick health check
curl -s https://rinkuchan.com/api/sync/status | jq

# Compare checkpoint heights across nodes
for node in "https://rinkuchan.com" "http://localhost:3001"; do
  echo "$node:"
  curl -s "$node/api/checkpoints" | jq '.chain | length'
done

# Watch transaction count in real-time
watch -n 5 'curl -s http://localhost:3001/api/sync/status | jq .dagSize'
```
