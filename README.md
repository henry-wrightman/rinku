# Rinku - DAG-Based Distributed Ledger For Mesh-Native Systems

[![Rust CI](https://github.com/rinku-ledger/rinku/actions/workflows/rust.yml/badge.svg)](https://github.com/rinku-ledger/rinku/actions/workflows/rust.yml)
[![Node.js CI](https://github.com/rinku-ledger/rinku/actions/workflows/node.js.yml/badge.svg)](https://github.com/rinku-ledger/rinku/actions/workflows/node.js.yml)
[![Network Health](https://github.com/rinku-ledger/rinku/actions/workflows/network-health.yml/badge.svg)](https://github.com/rinku-ledger/rinku/actions/workflows/network-health.yml)
[![Protocol](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Frinku-genesis.fly.dev%2Fapi%2Fversion&query=%24.protocolVersion&label=protocol&color=blue&cacheSeconds=300)](https://rinkuchan.com)
[![Checkpoints](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Frinku-genesis.fly.dev%2Fapi%2Fnetwork%2Fstats&query=%24.checkpointCount&label=checkpoints&color=blue&cacheSeconds=60)](https://rinkuchan.com)
[![Finality](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Frinku-genesis.fly.dev%2Fapi%2Fnetwork%2Fstats&query=%24.finalityRatio&label=finality&color=brightgreen&suffix=%25&cacheSeconds=60)](https://rinkuchan.com)

**Testnet nodes:**
[![Genesis](https://img.shields.io/website?url=https%3A%2F%2Frinku-genesis.fly.dev%2Fapi%2Fhealth&label=genesis&up_message=online&down_message=offline&up_color=brightgreen&down_color=red)](https://rinku-genesis.fly.dev/api/health)
[![Validator-1](https://img.shields.io/website?url=https%3A%2F%2Frinku-validator-1.fly.dev%2Fapi%2Fhealth&label=validator-1&up_message=online&down_message=offline&up_color=brightgreen&down_color=red)](https://rinku-validator-1.fly.dev/api/health)
[![Validator-2](https://img.shields.io/website?url=https%3A%2F%2Frinku-validator-2.fly.dev%2Fapi%2Fhealth&label=validator-2&up_message=online&down_message=offline&up_color=brightgreen&down_color=red)](https://rinku-validator-2.fly.dev/api/health)

---

A DAG-based distributed ledger with tunable consistency, designed for mesh-native and partition-prone environments. Delivers CP-like checkpoint finality during normal operation, provisional availability during partitions, and deterministic merge reconciliation when connectivity is restored. Self-contained VerifiableObject proofs enable offline verification without RPC infrastructure.

## Quick Start

### Prerequisites

- **Rust** (1.75+) - Install via [rustup](https://rustup.rs/)
- **Node.js** (18+) - For the explorer and faucet

### Build the Node

```bash
# Clone and build
git clone <repo-url>
cd rinku

# Build the Rust node
cargo build -p rinku-node --release
```

### Run Locally (Standalone Genesis Node)

```bash
# Start a fresh local node
RUST_LOG=info cargo run -p rinku-node

# Or run in TUI mode (terminal interface)
RUST_LOG=info cargo run -p rinku-node --features tui -- --tui
```

---

## Connecting to Testnet via TUI

To sync your local node with the live Fly.io testnet validators:

### Step 1: Get Bootstrap Info from Testnet

```bash
# Get the P2P bootstrap info from the genesis node
curl https://rinku-genesis.fly.dev/api/bootstrap
```

This returns the peer ID and multiaddr needed to connect via P2P.

### Step 2: Run Local Node with Testnet Peers

From the bootstrap response, extract these values:
- `peerId` — the genesis node's libp2p peer ID
- `genesisValidatorEnv` — the `address:blsPublicKey` string for `GENESIS_VALIDATORS`

Then look up the genesis node's public IPv4 (shown in the bootstrap response's `bootstrapMultiaddr` field, or via `fly ips list -a rinku-genesis`).

```bash
# Required: P2P connection to the testnet
export P2P_BOOTSTRAP_PEERS="/ip4/<GENESIS_IP>/tcp/4001/p2p/<PEER_ID>"

# Required: Trust anchor — tells your node which validators are authorized
# Use the genesisValidatorEnv value from /api/bootstrap
# For multiple validators, separate with semicolons: "addr1:bls1;addr2:bls2;addr3:bls3"
export GENESIS_VALIDATORS="<ADDRESS>:<BLS_PUBLIC_KEY>"

# Required: Must match the testnet's chain/network identity
export CHAIN_ID="rinku-testnet"
export NETWORK_ID="testnet"

# Required: Mainnet mode enforces strict validation (the testnet runs with this enabled)
export MAINNET_MODE="true"

# Required: Your node's reachable URL (used for leader election protocol)
export PUBLIC_URL="http://localhost:3001"

# Optional: HTTP peer for fallback sync (in addition to P2P)
export NODE_PEERS="https://rinku-genesis.fly.dev"

# Logging
export RUST_LOG="rinku_node=info"

# Run in TUI mode
cargo run -p rinku-node --features tui -- --tui
```

Or as a single command:

```bash
P2P_BOOTSTRAP_PEERS="/ip4/<GENESIS_IP>/tcp/4001/p2p/<PEER_ID>" \
GENESIS_VALIDATORS="<ADDRESS>:<BLS_PUBLIC_KEY>" \
CHAIN_ID="rinku-testnet" \
NETWORK_ID="testnet" \
MAINNET_MODE="true" \
PUBLIC_URL="http://localhost:3001" \
NODE_PEERS="https://rinku-genesis.fly.dev" \
RUST_LOG="rinku_node=info" \
cargo run -p rinku-node --features tui -- --tui
```

**Important notes:**
- Use `/ip4/` (not `/dns4/`) when the bootstrap address is a raw IP
- `CHAIN_ID` and `NETWORK_ID` must match the testnet — mismatches cause handshake rejection
- Without `GENESIS_VALIDATORS`, your node cannot verify checkpoint signatures during sync
- `IS_GENESIS_NODE` is auto-detected as `false` when `P2P_BOOTSTRAP_PEERS` is set

### Step 3: Verify Sync

In the TUI, you should see:
- Checkpoint height increasing as you sync
- DAG size growing
- Peer count > 0 once connected

Or via API:
```bash
curl http://localhost:3001/api/sync/status
curl http://localhost:3001/api/dag/summary
```

### Testnet Nodes

| Node | URL | Purpose |
|------|-----|---------|
| Genesis | https://rinku-genesis.fly.dev | Primary testnet node |
| Validator 1 | https://rinku-validator-1.fly.dev | Validator node |
| Validator 2 | https://rinku-validator-2.fly.dev | Validator node |

---

## Environment Variables

### Core Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `API_PORT` | HTTP API port | `3001` |
| `DATA_DIR` | Database storage path | `.rinku-data` |
| `CHAIN_ID` | Chain identifier (must match network) | `rinku-mainnet` |
| `NETWORK_ID` | Network identifier | `mainnet` |
| `RUST_LOG` | Log level (debug, info, warn, error) | `info` |
| `IS_GENESIS_NODE` | Allow creating new chain if no peers | auto-detected |

### P2P Networking

| Variable | Description | Default |
|----------|-------------|---------|
| `P2P_ENABLED` | Enable libp2p networking | `true` |
| `P2P_PORT` | P2P listen port | `4001` |
| `P2P_BOOTSTRAP_PEERS` | Comma-separated multiaddrs | `""` |
| `P2P_MDNS` | Enable mDNS for LAN discovery | `true` |
| `P2P_MAX_PEERS` | Maximum peer connections | `50` |

### HTTP Sync

| Variable | Description | Default |
|----------|-------------|---------|
| `NODE_PEERS` | Comma-separated HTTP peer URLs | `""` |

### Validator Mode

| Variable | Description | Default |
|----------|-------------|---------|
| `MAINNET_MODE` | Enforce mainnet-grade security | `false` |
| `PUBLIC_URL` | Node's public URL for leader election | `""` |
| `GENESIS_VALIDATORS` | Genesis validator set (addr:bls;...) | `""` |
| `VALIDATOR_KEY_PASSWORD` | Validator key passphrase | `""` |

---

## Wallet Key Management

The node generates an ECDSA P-256 wallet on first startup, stored encrypted at `<DATA_DIR>/validator.key`. This wallet holds RKU, stakes, and receives staking rewards.

### Export Your Node's Wallet Key

```bash
# Show the wallet address only
cargo run -p rinku-node -- --show-address

# Export wallet JSON (compatible with Explorer import)
VALIDATOR_KEY_PASSWORD="your-password" cargo run -p rinku-node -- --export-key
```

The wallet JSON is printed to stdout (instructions go to stderr), so you can pipe it:
```bash
VALIDATOR_KEY_PASSWORD="your-password" cargo run -p rinku-node -- --export-key > my-wallet.json
```

The exported JSON contains `publicKey`, `privateKey` (PKCS8 DER hex), and `fingerprint` — the same format used by the Explorer wallet.

### Import a Wallet Key Into a Node

```bash
# Import from wallet JSON (exported from Explorer or another node)
VALIDATOR_KEY_PASSWORD="your-password" cargo run -p rinku-node -- --import-key '{"publicKey":"04...","privateKey":"3081...","fingerprint":"..."}'

# Import from PKCS8 DER hex
VALIDATOR_KEY_PASSWORD="your-password" cargo run -p rinku-node -- --import-key 308187020100...

# Import from raw 32-byte private key hex
VALIDATOR_KEY_PASSWORD="your-password" cargo run -p rinku-node -- --import-key 368e9a5471...
```

This lets you use the same wallet identity across the Explorer and your validator node. The key is encrypted with your `VALIDATOR_KEY_PASSWORD` before being saved.

**Note:** In `MAINNET_MODE`, `VALIDATOR_KEY_PASSWORD` must be explicitly set (the default `dev-password` is rejected).

### Key File Location

| File | Contents |
|------|----------|
| `<DATA_DIR>/validator.key` | Encrypted ECDSA private key (wallet for RKU transactions) |
| `<DATA_DIR>/validator-identity/validator_keys.json` | BLS signing keys (for checkpoint signatures) |

---

## TUI Mode

The TUI (Terminal User Interface) provides a real-time dashboard for monitoring node state:

```bash
# Build with TUI feature
cargo build -p rinku-node --features tui

# Run TUI
cargo run -p rinku-node --features tui -- --tui
```

TUI displays:
- Checkpoint height and DAG size
- Connected peers
- Transaction throughput
- Finality metrics
- Network status

---

## Explorer

The web-based block explorer runs on port 5000:

```bash
# Build core library and start explorer
npm run build -w @rinku/core
npm run dev -w @rinku/explorer
```

Visit http://localhost:5000 to view:
- DAG visualization
- Account balances
- Transaction history
- Staking interface
- Faucet

---

## API Endpoints

The node exposes a REST API on port 3001:

| Endpoint | Description |
|----------|-------------|
| `GET /api/dag/summary` | DAG statistics |
| `GET /api/accounts` | All accounts |
| `GET /api/account/:addr` | Account details |
| `GET /api/tx/:hash` | Transaction details |
| `POST /api/tx` | Submit transaction |
| `GET /api/sync/status` | Sync status |
| `GET /api/bootstrap` | P2P bootstrap info |
| `GET /api/peers` | Connected peers |
| `GET /api/finality/metrics` | Finality statistics |

---

## Project Structure

```
rinku/
├── packages/
│   ├── rinku-core/      # Core types, crypto, merkle trees (Rust)
│   ├── rinku-node/      # Full node implementation (Rust)
│   ├── core/            # TypeScript core library
│   ├── explorer/        # React block explorer
│   └── faucet/          # Testnet faucet
├── scripts/             # Deployment and testing scripts
├── fly.toml             # Fly.io deployment config
└── Cargo.toml           # Rust workspace config
```

---

## Development

### Run All Services Locally

```bash
# Terminal 1: Rust node
RUST_LOG=info cargo run -p rinku-node

# Terminal 2: Explorer (port 5000)
npm run build -w @rinku/core && npm run dev -w @rinku/explorer

# Terminal 3: Faucet
npm run dev -w @rinku/faucet
```

### Run Tests

```bash
# Rust tests
cargo test

# TypeScript tests
npm test
```

---

## License

MIT
