# Rinku

Rinku is a URL-native distributed ledger with DAG-based consensus. This repo includes the Rust node implementation, a core TS library, a web explorer, a faucet, and supporting tooling.

## Requirements

- Rust (stable, via `rustup`)
- Node.js 18+ and npm
- Optional: Fly.io CLI (`flyctl`) for deployment

## Quick start

```bash
npm install
```

## Running a node (local)

The node is implemented in Rust under `packages/rinku-node`. You can run it with defaults or configure via environment variables.

```bash
RINKU_PORT=3001 \
RINKU_DB_PATH=./rinku-data \
RUST_LOG=info \
cargo run -p rinku-node
```

### Connect to the current Fly.io testnet

The current testnet is hosted on Fly.io nodes. To join it, use any Fly node as a bootstrap source.

1. Pick a Fly node URL (ask the team for the current list).
2. Fetch bootstrap info:

```bash
curl https://<fly-node>.fly.dev/api/bootstrap
```

(current fly nodes as of 1/28/25)  
https://rinku-genesis.fly.dev  
https://rinku-validator-1.fly.dev  
https://rinku-validator-2.fly.dev  

3. Start your node with the returned bootstrap values (replace `<PUBLIC_IP>` in the
   `bootstrap_multiaddr` with the Fly node's public IP):

```bash
CHAIN_ID=rinku-testnet \
NETWORK_ID=testnet \
GENESIS_VALIDATORS="<addr:bls_pubkey from genesis_validator_env>" \
P2P_BOOTSTRAP_PEERS="/ip4/<PUBLIC_IP>/tcp/4001/p2p/<PEER_ID>" \
API_PORT=3001 \
P2P_PORT=4001 \
cargo run -p rinku-node
```

You can verify connectivity with:

```bash
curl http://localhost:3001/api/sync/status
```

### Node configuration

Common environment variables:

- `RINKU_PORT` — HTTP API port (default: 3001)
- `NODE_PEERS` — Comma-separated list of peer URLs
- `RINKU_DB_PATH` — Database path (default: `./rinku-data`)
- `RUST_LOG` — Log level (default: `info`)
- `MAINNET_MODE` — Enforce mainnet-grade checks (default: `false`)
- `GENESIS_VALIDATORS` — Bootstrap validators (`addr:bls_pubkey;...`)
- `PUBLIC_URL` — Node URL for leader election
- `CHAIN_ID` — Chain identifier (default: `rinku-testnet`)
- `NETWORK_ID` — Network identifier (default: `testnet`)

For a complete multi-node testnet guide, see `scripts/TESTNET_SETUP.md`.

### Local multi-node testnet

```bash
./scripts/local-testnet.sh start 3
./scripts/local-testnet.sh status
./scripts/local-testnet.sh validate
./scripts/local-testnet.sh stop
```

### TUI (terminal UI)

The node has a TUI mode behind the `tui` feature flag.

```bash
cargo run -p rinku-node --features tui -- --tui
```

Flags:

- `--tui` or `-t` — enable the terminal UI

When TUI is enabled, logs are written to `.rinku-data/tui.log` to avoid corrupting the UI.

## Running tests

### JS/TS packages

```bash
npm run test
```

### Rust (node and core)

```bash
cargo test -p rinku-node
cargo test -p rinku-core
```

## Useful scripts

```bash
# Start the explorer dev server
npm run dev:explorer

# Start the faucet dev server
npm run dev:faucet

# Validate a running testnet
npm run validate

# Run activity bot against a node
RINKU_NODE_URL=http://localhost:3001 npm run activity-bot
```

## Docs

- `packages/rinku-node/CONSENSUS.md` — consensus protocol notes
- `WHITEPAPER.md` — full protocol writeup (and ai slop)
- `rinku.pdf` - self-provable units which are kinda sick
