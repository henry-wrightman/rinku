# Rinku - URL-Native Distributed Ledger

## Overview
Rinku is a URL-native distributed ledger project that utilizes DAG-based consensus and weight-based Sybil resistance. Its core innovation is enabling the entire ledger state to exist as cryptographically-linked URLs, allowing for a self-crawlable and verifiable chain without traditional node infrastructure. The project aims to create a highly decentralized, robust, and trustlessly verifiable distributed ledger, pioneering a new approach to blockchain interaction and verification. Key features include a hard-capped token supply, dynamic gas fees, and optional ZK privacy, offering a new paradigm for distributed ledger technology.

## User Preferences
I want to work iteratively. Please ask before making major changes. I prefer detailed explanations for complex features.

## System Architecture

### UI/UX Decisions
- **Explorer:** A React-based block explorer for visualizing the DAG, accounts, faucet, contracts, and staking.
- **In-Browser Wallet:** A built-in wallet generates ECDSA P-256 keypairs using Web Crypto API, with client-side transaction signing.

### System Design Choices
- **URL-Native Ledger:** The entire ledger state is embedded in cryptographically linked URLs, making the chain self-crawlable and verifiable. Transactions are base64url-encoded, deflated JSON objects in URLs.
- **DAG-Based Consensus:** Transactions reference 1-2 global DAG "tips" (recent unconfirmed transactions), weaving all activity into a shared directed acyclic graph. Nonces provide per-account ordering. Conflicts are resolved by cumulative weight for Sybil resistance.
- **Trustless Verification:** Finality proofs, embedded as URL query parameters, allow complete cryptographic validation of transactions and ledger state directly from URLs.
- **Smart Contracts:** Contract code and state are URL-encoded, with calls embedded within transactions.
- **Reward and Staking System:** Supports Tip, Stake, and Witness Rewards, with a staking mechanism for validators including slashing and an unbonding queue.
- **Dynamic Gas Fee Model (EIP-1559 Style):** Utilization-based pricing with an adaptive fee split (70%+ to validators, up to 30% burned).
- **Tokenomics System:** Hard-capped supply (30M RKU), genesis allocation, checkpoint-based emission with 18-month halving epochs, and WPoS reward distribution.
- **Multi-Node Networking:** Gossip protocol for peer discovery and a snapshot-based sync protocol for efficient state synchronization. Nodes perform periodic peer sync and dynamic peer discovery.
- **Snapshot-Based Sync Architecture:** New nodes sync via state snapshots (accounts, validators, checkpoints, recent DAG) instead of full transaction history, optimizing sync time and bandwidth.
- **Performance Optimizations:** Includes in-memory DAG pruning, snapshot optimizations, checkpoint-bounded self-crawlable URLs, per-transaction finality, self-contained Merkle proofs, batched operations, parallel signature verification, and batch transaction API.
- **Memory Management:** Bounded hash sets with FIFO eviction prevent memory leaks.
- **Gossip Bandwidth Optimization:** TipAnnouncement messages cap tips to prevent bandwidth explosion.
- **Server-Side Tip Injection & Protocol-Level Tip Consolidation:** Mechanisms to manage and reduce DAG tips.
- **BLS Signature Aggregation:** Uses BLS12-381 for compact checkpoint validator signatures.
- **Compact Proof Format:** Self-contained finality proofs designed for size efficiency, utilizing DEFLATE compression and base64url encoding.
- **Self-Contained Proof System (v5 - MerkleSumTree Multi-Proof):** Fully offline-verifiable transaction proofs with chain identity binding.
- **ZK Privacy Layer:** Optional privacy-preserving proofs using Groth16 ZK-SNARKs for transactions.
- **Protocol Versioning & Upgrades:** Semantic versioning, feature flags, upgrade proposals, and peer compatibility checks.

### Transaction Validation & Security
All transactions undergo comprehensive validation, including account existence, balance, nonce, and gas fee checks. Production APIs (`/api/tx`, `/api/tx/batch`) enforce full validation with pre-validation checks (balance + gas) before any state mutations.

**All state-changing operations must go through the signed transaction flow:**
- Staking: Submit a transaction with `kind: "stake"` via `/api/tx`
- Rewards claiming: Submit a transaction with `kind: "claim_rewards"` via `/api/tx`
- Contract deployment: Submit a transaction with `kind: "deploy_contract"` via `/api/tx`
- Contract calls: Submit a transaction with `kind: "call_contract"` via `/api/tx`

Read-only endpoints (`GET /api/staking/:address`, `GET /api/rewards/:address`, `GET /api/contracts/:id`) remain available for querying state without authentication.

### Scalable State Architecture (Phase 2 - January 2026)
- **Persistent Storage:** Migrated from sled to redb (pure Rust, ACID, MVCC) with separate tables for accounts, DAG, checkpoints, validators, trie, metadata, contracts, rewards
- **JSON Serialization:** Using serde_json for compatibility with complex serde features (#[serde(flatten)], etc.)
- **DAG Pruning:** Checkpoint-bounded retention, configurable 100+ checkpoint window
- **Sparse Merkle Trie:** 256-level trie for verifiable state roots with O(log n) proofs
- **Sharding Design:** Account-based sharding strategy documented for billions of accounts (see SHARDING.md)

### Technical Implementations
- **Core Library:** Shared library for types, cryptography (Web Crypto API), encoding, Merkle trees, DAG structures, and weight calculation.
- **Fork Remediation Service:** Double-spend detection only. Multiple DAG tips are treated as healthy concurrent branches (not forks). Checkpointing naturally merges tips into finalized state. Only actual conflicts (same account + same nonce) trigger branch pruning.
- **Pre-Sync Flush:** Before snapshot sync, nodes broadcast local transactions to peers to prevent transaction loss during state replacement.
- **API Rate Limiting:** Tiered rate limiting for various endpoints.
- **Prometheus Metrics:** `/metrics` endpoint for network monitoring.
- **Proof Verification API:** `POST /api/verify-proof` for decoding and verifying self-contained proof URLs.
- **Testnet Tooling:** Scripts for proof generation/verification, size benchmarking, multi-node validation, and local testnet orchestration.

### Multi-Node Consensus Model
Consensus is maintained across nodes by ensuring critical metrics like checkpoint Merkle roots and account balances match. Fork prevention mechanisms involve validating peer checkpoints before adoption, and automatic fork recovery triggers after consistent mismatches, involving snapshot retrieval and state replacement.

### Sync Trust Model
**Snapshot Sync**: MERGES accounts from peer into local state (preserving accounts unique to each node). Used for initial sync or recovery. For accounts that exist on both nodes, keeps the one with higher nonce (more transactions processed). Includes complete state:
- Accounts (balances, nonces, stakes) - MERGED, not replaced
- Validators
- Checkpoints
- Smart contracts (code + state)
- Rewards service (staking positions, pending rewards)
- Emission service (total emitted, burned)
- Slashing service (slash history, unbonding queue)
- Gas tracking (total_burned, total_to_validators)

**Delta Sync**: Fetches transactions since last checkpoint. Uses `add_transaction_dag_only()` which skips nonce validation (since account nonces are synced first) but verifies DAG parent existence.

**Account State Verification**: During periodic sync, nodes compare faucet balances. If checkpoints match but balances diverge by more than 1.0 RKU, triggers bidirectional account merge sync to ensure all accounts from both nodes are present.

Both sync methods trust the connected peer. For adversarial environments:
- Use checkpoint verification (BLS aggregate signatures)
- Implement peer allowlists
- Use stake-weighted trust for peer selection

**Normal gossip** for new transactions uses full validation including nonce/balance checks.

### Trust Bootstrap System
A hybrid trust model combines genesis validators and stake-weighted verification. Environment variables configure genesis validators, quorum thresholds, and weak subjectivity checkpoints for fast bootstrapping. Testnet mode operates without genesis validators, performing only BLS format validation.

### Test Coverage
Comprehensive test coverage includes 89 unit tests and 20 multi-node integration tests:
- **Merkle Proofs:** Verified compatibility between `verify_tx_merkle_proof` and core `MerkleTree` builder, covering multi-leaf and odd-sized trees
- **Consensus:** Fork detection, snapshot sync, checkpoint adoption, fork resolution, delta sync, network partitions
- **Cryptography:** BLS signatures, P-256 ECDSA, SHA-256 hashing
- **State Management:** Persistence, state trie, validator management, slashing, unbonding
- **Protocol:** Version compatibility, trust verification, checkpoint chain validation
- **P2P Networking:** Bloom filter operations, DoS protection, handshake validation, sync verification

### P2P Networking Architecture (Phase 3 - January 2026)
The Rust node implements a production-grade P2P networking layer using libp2p for secure, decentralized communication:

**Transport & Security:**
- libp2p with TCP transport and Noise protocol encryption
- Yamux multiplexing for efficient stream management
- mDNS for automatic local peer discovery
- Bootstrap peer configuration for network joining

**Messaging Layer:**
- GossipSub for efficient pub/sub message propagation
- CBOR-serialized request/response protocol for sync operations
- Message types: Transaction, TipAnnouncement, BlockProposal, BloomAnnouncement

**Request/Response Protocol:**
- Snapshot sync: Full state transfer (accounts, validators, checkpoints)
- Delta sync: Transactions since specific checkpoint height
- Transaction requests: Fetch specific transactions by hash
- Proof requests: Request merkle proofs for transaction verification
- Account state queries: Fetch account data with merkle verification
- Authenticated handshake: Protocol version, chain/network ID validation

**DoS Protection:**
- Connection limits: Max 50 connections per peer (configurable)
- Rate limiting: Token bucket algorithm (10 tokens/sec, 20 burst)
- Peer banning: 300s default ban duration for misbehavior
- Sync/async variants for non-blocking validation

**Bloom Filter Announcements:**
- 524,288 bit filters (64KB) with 7 hash functions
- Double hashing: h(i) = (h1 + i*h2) mod m
- ~1% false positive rate for 50K items, ~12% for 100K items (use compact filters for smaller sets)
- Bandwidth-efficient transaction advertising between peers
- Automatic filter generation from local DAG tips

**Verified Sync with Merkle Proofs:**
- Snapshot verification: Validate received state against merkle root
- Account proofs: Generate and verify merkle proofs for individual accounts
- SyncVerifier: Strict/lenient modes for verification enforcement
- Tamper detection: Any modification invalidates proofs

**Key Files:**
- `packages/rinku-node/src/network.rs` - NetworkService, libp2p setup, P2P event handling
- `packages/rinku-node/src/gossip.rs` - GossipService, BloomFilter, message broadcasting
- `packages/rinku-node/src/sync_verification.rs` - Merkle proof verification for sync
- `packages/rinku-node/tests/p2p_integration.rs` - P2P integration tests

### E2E Multi-Node Testing (Phase 3.5 - January 2026)
Comprehensive end-to-end test suite for multi-node P2P network validation:

**Test Infrastructure:**
- `spawn_test_node()` helper to spawn real libp2p nodes on different ports
- NetworkHandle extensions: `connect()`, `stats()`, `broadcast()`, `request_snapshot()`
- Async tokio-based tests with proper assertions

**E2E Test Coverage:**
- `test_e2e_node_connection`: Two-node connection establishment
- `test_e2e_bloom_broadcast`: Bloom filter gossip propagation
- `test_e2e_three_node_mesh`: Three-node mesh formation and connectivity
- `test_e2e_sync_request`: Request/response sync protocol validation
- `test_e2e_peer_stats`: Peer counting and connection statistics

### TPS Benchmarking Infrastructure (Phase 3.5 - January 2026)
Production benchmarking module for measuring transaction throughput:

**Benchmark Components:**
- `BenchmarkConfig`: Configurable tx_count, workers, batch_size, warmup, target_tps
- `LatencyTracker`: Collects timing metrics with safe percentile calculation (p50, p95, p99)
- `ThroughputMeter`: Real-time TPS monitoring with instant and overall rates
- `TxGenerator`: Atomic nonce-incrementing transaction generator
- `BenchmarkRunner`: Orchestrates benchmarks with success/error tracking

**Benchmark Results (January 2026):**
- Signature validation: **65,107 TPS** (P-256 ECDSA verification)
- Merkle tree operations: **877 TPS** for 100-leaf trees
- Target: 1,000-10,000+ TPS at mainnet scale

**Key Files:**
- `packages/rinku-node/src/benchmark.rs` - TPS benchmarking module

### Fly.io Deployment
The Rust node is deployable to Fly.io using a `Dockerfile.fly` and `fly.toml` for production environments.

## External Dependencies
- **Monorepo Management:** Cargo workspaces (Rust), npm workspaces (TypeScript).
- **Frontend:** React, Vite.
- **Backend:** Rust node (Axum) for consensus/API, Express (for TypeScript faucet).
- **Cryptography:** Web Crypto API, @noble/curves (BLS12-381), @noble/hashes (SHA-256).
- **Compression:** pako (DEFLATE).
- **Testing:** vitest.
- **ZK-SNARKs:** circomlib, snarkjs, circomlibjs.
- **Rust Libraries:** `p256`, `sha2`, `petgraph`, `tokio`, `axum`, `serde`, `serde_json`, `flate2`, `redb`, `tower-http`, `tracing`, `libp2p` (with gossipsub, request-response, mDNS, identify).