# Rinku - URL-Native Distributed Ledger

## Overview
Rinku is a pioneering URL-native distributed ledger project employing DAG-based consensus and weight-based Sybil resistance. Its primary innovation is embedding the entire ledger state within cryptographically-linked URLs, enabling a self-crawlable and trustlessly verifiable chain without reliance on traditional node infrastructure. The project aims to deliver a highly decentralized, robust, and verifiable distributed ledger, redefining interaction and verification within blockchain technology. Key features include a hard-capped token supply, dynamic gas fees, and optional ZK privacy, offering a novel paradigm for DLT.

## User Preferences
I want to work iteratively. Please ask before making major changes. I prefer detailed explanations for complex features.

## System Architecture

### UI/UX Decisions
- **Explorer:** A React-based block explorer visualizes the DAG, accounts, faucet, contracts, and staking.
- **In-Browser Wallet:** A built-in wallet generates ECDSA P-256 keypairs and performs client-side transaction signing using the Web Crypto API.

### System Design Choices
- **URL-Native Ledger:** The entire ledger state resides in cryptographically linked URLs, making the chain self-crawlable and verifiable. Transactions are base64url-encoded, deflated JSON objects within URLs.
- **DAG-Based Consensus:** Transactions reference 1-2 global DAG "tips," forming a shared directed acyclic graph. Conflicts are resolved via cumulative weight for Sybil resistance.
- **Trustless Verification:** Finality proofs, embedded in URL query parameters, allow complete cryptographic validation of transactions and ledger state directly from URLs.
- **Smart Contracts:** Contract code and state are URL-encoded, with calls embedded within transactions.
- **Reward and Staking System:** Supports Tip, Stake, and Witness Rewards, with a staking mechanism for validators including slashing and an unbonding queue.
- **Dynamic Gas Fee Model (EIP-1559 Style):** Utilization-based pricing with an adaptive fee split.
- **Tokenomics System:** Hard-capped supply (30M RKU), genesis allocation, checkpoint-based emission with 18-month halving epochs, and WPoS reward distribution.
- **Multi-Node Networking:** Gossip protocol for peer discovery and a snapshot-based sync protocol for efficient state synchronization.
- **Snapshot-Based Sync Architecture:** New nodes sync via state snapshots (accounts, validators, checkpoints, recent DAG) instead of full transaction history.
- **Performance Optimizations:** Includes in-memory DAG pruning, snapshot optimizations, checkpoint-bounded self-crawlable URLs, per-transaction finality, self-contained Merkle proofs, batched operations, parallel signature verification, and a batch transaction API.
- **BLS Signature Aggregation:** Uses BLS12-381 for compact checkpoint validator signatures.
- **Compact Proof Format:** Self-contained finality proofs are designed for size efficiency, utilizing DEFLATE compression and base64url encoding.
- **Self-Contained Proof System (MerkleSumTree Multi-Proof):** Fully offline-verifiable transaction proofs with chain identity binding.
- **ZK Privacy Layer:** Optional privacy-preserving proofs using Groth16 ZK-SNARKs for transactions.
- **Protocol Versioning & Upgrades:** Semantic versioning, feature flags, upgrade proposals, and peer compatibility checks.
- **Scalable State Architecture:** Persistent storage using redb, JSON serialization with serde_json, checkpoint-bounded DAG pruning, and a 256-level Sparse Merkle Trie for verifiable state roots. Account-based sharding is designed for billions of accounts.

### Technical Implementations
- **Core Library:** Shared library for types, cryptography, encoding, Merkle trees, DAG structures, and weight calculation.
- **Fork Remediation Service:** Double-spend detection; non-conflicting DAG tips are treated as healthy concurrent branches.
- **Pre-Sync Flush:** Nodes broadcast local transactions to peers before snapshot sync to prevent data loss.
- **API Rate Limiting:** Tiered rate limiting for various endpoints.
- **Prometheus Metrics:** `/metrics` endpoint for network monitoring.
- **Proof Verification API:** `POST /api/verify-proof` for decoding and verifying self-contained proof URLs.
- **Testnet Tooling:** Scripts for proof generation/verification, size benchmarking, multi-node validation, and local testnet orchestration.
- **Multi-Node Consensus Model:** Consensus is maintained by matching critical metrics like checkpoint Merkle roots and account balances. Fork prevention involves validating peer checkpoints, and automatic recovery triggers after consistent mismatches.
- **Sync Trust Model:** Snapshot sync merges accounts from peers into local state. Delta sync fetches transactions since the last checkpoint. Both trust the connected peer, with options for checkpoint verification, peer allowlists, and stake-weighted trust for adversarial environments.
- **Genesis Hash Validation:** Nodes validate genesis hash before accepting sync snapshots, preventing contamination from stale deployments or different chains. This ensures fresh Fly.io deployments won't sync data from old Replit deployments with different genesis.
- **Local Multi-Node Testnet:** Scripts for local testnet orchestration (`scripts/local-testnet.sh`) with automatic port allocation (3011+/4011+), transaction propagation testing (10 accounts per node), and multi-node validation.
- **Trust Bootstrap System:** A hybrid trust model combines genesis validators and stake-weighted verification, configurable via environment variables.
- **P2P Networking Architecture:** Implements a production-grade P2P layer using libp2p with TCP transport, Noise encryption, Yamux multiplexing, mDNS, and Bootstrap peer configuration.
- **Messaging Layer:** Uses GossipSub for pub/sub and CBOR-serialized request/response for sync operations. Message types include Transaction, TipAnnouncement, BlockProposal, and BloomAnnouncement.
- **DoS Protection:** Includes connection limits, rate limiting via token bucket algorithm, and peer banning for misbehavior.
- **Bloom Filter Announcements:** Uses 524,288 bit filters with 7 hash functions for bandwidth-efficient transaction advertising.
- **Verified Sync with Merkle Proofs:** Snapshot verification, account proofs, and a `SyncVerifier` for tamper detection.
- **Slashing-Consensus Integration:** `ConsensusService` integrates with `SlashingService` for unified slashing, double-sign detection, and automatic slashing based on LIVENESS_MISS_THRESHOLD. `track_liveness()` is called after each checkpoint creation with participating validator addresses.
- **DoS Protection Enforcement:** Rate limiting enforced for gossip messages and sync requests, with automatic rejection and peer banning for violations. Conservative approach rejects on lock contention rather than bypassing.
- **DAG Pruning Scheduling:** `CheckpointService` integrates `DagPruningService`, with `prune_dag()` called via `NodeState.storage()` getter. Pruning triggered every 10 checkpoints after height 100, logging stats (nodes/checkpoints pruned, oldest retained).
- **Multi-Validator Quorum Collection:** `collect_validator_quorum()` gathers BLS signatures from peers with fallback to single-validator mode. P2P vote protocol implemented with security: validates votes against local validator registry, verifies BLS keys match known validators, uses locally-known stake weights (not peer-supplied), and verifies signatures using decoded bytes. Supports stake-weighted quorum (2/3 total stake).
- **Fly.io Deployment:** The Rust node is deployable to Fly.io using a `Dockerfile.fly` and `fly.toml`.

## External Dependencies
- **Monorepo Management:** Cargo workspaces (Rust), npm workspaces (TypeScript).
- **Frontend:** React, Vite.
- **Backend:** Rust node (Axum) for consensus/API, Express (for TypeScript faucet).
- **Cryptography:** Web Crypto API, @noble/curves (BLS12-381), @noble/hashes (SHA-256).
- **Compression:** pako (DEFLATE).
- **Testing:** vitest.
- **ZK-SNARKs:** circomlib, snarkjs, circomlibjs.
- **Rust Libraries:** `p256`, `sha2`, `petgraph`, `tokio`, `axum`, `serde`, `serde_json`, `flate2`, `redb`, `tower-http`, `tracing`, `libp2p` (gossipsub, request-response, mDNS, identify).

## Testnet Deployment Guide

### Architecture
- **Fly.io**: Runs Rust nodes (genesis + validators) with full P2P consensus
- **Replit**: Hosts explorer frontend connecting to Fly.io node API

> Note: Replit deployments only support a single external port, so P2P nodes must run on Fly.io.

### Step 1: Deploy Genesis Node to Fly.io

```bash
# Create the app
fly apps create rinku-genesis

# IMPORTANT: Allocate dedicated IPv4 for P2P port 4001
fly ips allocate-v4 --app rinku-genesis

# Deploy the node
fly deploy --dockerfile Dockerfile.fly --app rinku-genesis

# Wait for startup, then get bootstrap info
curl https://rinku-genesis.fly.dev/api/bootstrap

# Get the public IP address for P2P connections
fly ips list --app rinku-genesis
```

Response will include:
- `peerId`: libp2p peer ID
- `bootstrapMultiaddr`: Template for P2P_BOOTSTRAP_PEERS
- `genesisValidatorEnv`: Format for GENESIS_VALIDATORS
- `genesisHash`: Unique chain identity (nodes reject sync from peers with different genesis hash)

### Step 2: Deploy Additional Validator Nodes

```bash
# Create new app for each validator
fly apps create rinku-validator-1

# Set bootstrap configuration
fly secrets set -a rinku-validator-1 \
  P2P_BOOTSTRAP_PEERS="/ip4/<GENESIS_IP>/tcp/4001/p2p/<PEER_ID>" \
  GENESIS_VALIDATORS="<ADDRESS>:<BLS_PUBLIC_KEY>"

# Deploy
fly deploy --dockerfile Dockerfile.fly --app rinku-validator-1
```

To get the genesis node's public IP:
```bash
fly ips list -a rinku-genesis
```

### Step 3: Deploy Explorer to Replit

Set environment variable before deploying:
```
VITE_API_URL=https://rinku-genesis.fly.dev
```

The explorer will proxy all `/api` requests to the Fly.io node.

### Verifying the Network

1. Check genesis node status:
   ```bash
   curl https://rinku-genesis.fly.dev/api/status
   ```

2. Check validator node sync:
   ```bash
   curl https://rinku-validator-1.fly.dev/api/sync/status
   ```

3. Check P2P connectivity:
   ```bash
   curl https://rinku-genesis.fly.dev/api/bootstrap
   # Should show peer connections in network stats
   ```

### Environment Variables Reference

| Variable | Description | Required |
|----------|-------------|----------|
| `API_PORT` | HTTP API port (default: 8080) | No |
| `P2P_PORT` | libp2p P2P port (default: 4001) | No |
| `DATA_DIR` | Persistent data directory | Yes |
| `P2P_BOOTSTRAP_PEERS` | Multiaddr of bootstrap peer(s) | No (genesis) / Yes (validators) |
| `GENESIS_VALIDATORS` | Trusted validator addresses with BLS keys | No (genesis) / Yes (validators) |
| `RUST_LOG` | Log level filter | No |
| `PUBLIC_URL` | Public URL for gossip peer discovery | Yes (for Fly.io) |

### Automated Deployment Script

Use `scripts/fly-deploy.sh` for streamlined deployments:

```bash
# Update all nodes with new code (retains chain history)
./scripts/fly-deploy.sh update

# Fresh deployment (wipes all data, restarts genesis)
./scripts/fly-deploy.sh fresh

# Update only genesis or validators
./scripts/fly-deploy.sh update-genesis
./scripts/fly-deploy.sh update-validators

# Check network status
./scripts/fly-deploy.sh status

# Get bootstrap info for manual configuration
./scripts/fly-deploy.sh bootstrap-info

# View logs
./scripts/fly-deploy.sh logs rinku-genesis
```

The script handles:
- App creation and IPv4 allocation
- Volume management for fresh deployments
- Automatic bootstrap configuration for validators
- Sequential deployment with proper timing

### Leader Election

Checkpoint creation uses stake-weighted leader election to prevent state divergence in multi-node networks:
- Leader is deterministically selected using VRF-style randomness based on previous checkpoint hash
- Only the elected leader creates checkpoints; other nodes sync from peers
- Ensures consistent checkpoint creation across the network