# Rinku - URL-Native Distributed Ledger

## Overview
Rinku (Japanese for "link") is a URL-native distributed ledger project that utilizes DAG-based consensus and weight-based Sybil resistance. Its key innovation is enabling the entire ledger state to exist as cryptographically-linked URLs, allowing for a self-crawlable and verifiable chain without traditional node infrastructure. The project aims to create a highly decentralized, robust, and trustlessly verifiable distributed ledger, pioneering a new approach to blockchain interaction and verification. It features a hard-capped token supply, dynamic gas fees, and optional ZK privacy.

## User Preferences
I want to work iteratively. Please ask before making major changes. I prefer detailed explanations for complex features.

## System Architecture

### UI/UX Decisions
- **Explorer:** A React-based block explorer for visualizing the DAG, accounts, faucet, contracts, and staking.
- **In-Browser Wallet:** The Rewards/Staking tab includes a built-in wallet that generates ECDSA P-256 keypairs using Web Crypto API. Keys are cached in memory and can be persisted to localStorage. All transaction signing happens client-side - private keys never leave the browser.

### System Design Choices
- **URL-Native Ledger:** The entire ledger state is embedded in cryptographically linked URLs, making the chain self-crawlable and verifiable. Transactions are base64url-encoded, deflated JSON objects directly in URLs.
- **DAG-Based Consensus:** Accounts maintain micro-chains, with transactions referencing multiple prior "tips." Conflicts are resolved by cumulative weight, providing Sybil resistance.
- **Trustless Verification:** Finality proofs, embedded as URL query parameters, allow complete cryptographic validation of transactions and ledger state directly from URLs.
- **Smart Contracts:** Contract code and state are URL-encoded, with calls embedded within transactions. Contracts are persisted in node state and distributed across all nodes. API endpoints: `GET /api/contracts` (list all), `GET /api/contracts/:id` (get one), `POST /api/contracts/deploy` (deploy new), `POST /api/contracts/:id/call` (execute entrypoint with state mutation).
- **Reward and Staking System:** Supports Tip, Stake, and Witness Rewards, with a staking mechanism for validators including slashing penalties and an unbonding queue.
- **Dynamic Gas Fee Model (EIP-1559 Style):** Utilization-based pricing adjusts based on transaction volume, with an adaptive fee split (70%+ to validators, up to 30% burned).
- **Tokenomics System:** Hard-capped supply (30M RKU), genesis allocation, checkpoint-based emission with 18-month halving epochs, and WPoS reward distribution.
- **Multi-Node Networking:** Gossip protocol for peer discovery and snapshot-based sync protocol.
  - `POST /api/gossip`: Receives gossip messages (transactions, tips, sync requests) from peers
  - `GET /api/sync/status`: Returns node sync state (checkpoint height, DAG size, tips, merkle root)
  - `GET /api/sync/snapshot`: Snapshot-based sync - returns complete derived state (accounts, validators, checkpoints, recent DAG)
  - `GET /api/sync/transactions?hashes=a,b,c`: Batch fetch transactions by hash
  - `GET /api/sync/delta?from_checkpoint=N`: Fetch transactions since checkpoint N (used for continuous sync)
    - Supports pagination: `?from_checkpoint=N&limit=500&offset=0` returns structured response with `transactions`, `total`, `offset`, `limit`, `hasMore`
- **Periodic Peer Sync:** Nodes poll peer status every ~10 seconds and request missing transactions via delta sync endpoint
  - Paginated sync: fetches in batches of 500 transactions, iterates until caught up
- **Snapshot-Based Sync Architecture:** New nodes sync via state snapshots instead of full transaction history.
  - Transfers ~10KB (accounts + validators + checkpoints) instead of potentially GBs of transaction history
  - Self-contained URL proofs mean historical transactions aren't needed for verification
  - Snapshot includes: accounts, validators, checkpoints, gas_price, total_supply, genesis_time, recent DAG transactions
  - Fresh nodes rebuild DAG with synthetic genesis node, orphaned parents point to genesis
- **Performance Optimizations:** Includes in-memory DAG pruning, snapshot optimizations, checkpoint-bounded self-crawlable URLs, per-transaction finality, self-contained Merkle proofs, batched operations, parallel signature verification, and batch transaction API.
- **Memory Management:** Bounded hash sets for transaction tracking (50k max known_txs, 10k max seen_conflicts) with FIFO eviction to prevent memory leaks during continuous operation.
- **Protocol-Level Tip Consolidation:** Automatic DAG tip reduction via validator-created zero-fee consolidation transactions.
- **BLS Signature Aggregation:** Uses BLS12-381 for compact checkpoint validator signatures.
- **Compact Proof Format (Profile B Compact):** Self-contained finality proofs designed to fit in QR codes, utilizing DEFLATE compression and base64url encoding.
- **Self-Contained Proof System (v5 - MerkleSumTree Multi-Proof):** Fully offline-verifiable transaction proofs with chain identity binding, optimized for size reduction via multi-proofs.
- **ZK Privacy Layer:** Optional privacy-preserving proofs using Groth16 ZK-SNARKs, enabling `rinku://zk/{payload}` URLs for transactions without revealing sender, recipient, or amount.
- **Protocol Versioning & Upgrades:** Semantic versioning, feature flags with activation thresholds, upgrade proposals with validator signaling, and peer compatibility checks.

### Technical Implementations
- **Core Library:** Shared library for types, cryptography (Web Crypto API), encoding, Merkle trees, DAG structures, and weight calculation.
- **Fork Remediation Service:** Nonce-based double-spend detection, weight-based conflict resolution, and branch pruning.
- **API Rate Limiting:** Tiered rate limiting for TX, contract, and general endpoints.
- **Prometheus Metrics:** `/metrics` endpoint for network monitoring.
- **Proof Verification API:** `POST /api/verify-proof` endpoint decodes and verifies self-contained proof URLs, enabling offline verification of pruned transactions. Explorer includes a "Verify" tab for user-friendly proof validation.
- **Testnet Tooling (scripts/):**
  - `generate-proofs.ts`: Proof generation and verification pipeline using core functions (ECDSA via `verify`, Merkle via `verifyMerkleProof`, BLS via `verifySelfContainedProof`)
  - `proof-size-benchmark.ts`: Measures actual node-generated proof URL sizes for QR code compatibility
  - `validate-multi-node.ts`: Cross-node consensus validation - compares checkpoint merkle roots at common height, account balances
  - `local-testnet.sh`: Multi-node orchestration script
  - `TESTNET_SETUP.md`: 3-node testnet setup documentation

### Multi-Node Consensus Model
With snapshot-based sync and DAG pruning, nodes maintain consensus differently than traditional blockchains:

**Critical Consensus Metrics (MUST match):**
- Checkpoint merkle roots at the same height - proves identical transaction sets were finalized
- Account balances - derived state from all finalized transactions
- Total supply, validators, staking state

**Expected Differences (NOT consensus failures):**
- DAG transaction count - varies due to pruning timing and node age
- Tip count - changes constantly as transactions arrive
- DAG merkle root - changes with every transaction

**Validation Script:** Run `npx ts-node scripts/validate-multi-node.ts NODE1_URL NODE2_URL [NODE3_URL...]` to verify consensus. Critical failures (checkpoint root mismatch, balance mismatch) indicate a fork. Minor failures (sync lag) are normal during operation.

**Fork Prevention:** Before creating a checkpoint at height N, nodes query peers for existing checkpoints at that height via `GET /api/checkpoints/{height}`. If a peer has a checkpoint, the node validates it before adoption:
1. **Chain linkage**: Peer's `previous_hash` must match local last checkpoint
2. **Transaction set**: Peer's `tx_merkle_root` must match local pending transactions
3. **Checkpoint hash**: Peer's hash must match locally recomputed hash from checkpoint fields
4. **Signature validity**: At least one BLS signature must be parseable and well-formed (96 bytes)

If validation fails, the node attempts a delta sync to fetch missing transactions, recomputes the merkle root, and retries adoption. Only if all adoption attempts fail does the node create its own checkpoint. This prevents divergence when multiple nodes try to create checkpoints simultaneously.

**Fork Recovery:** After 3 consecutive `previous_hash` mismatches with peers, automatic fork recovery triggers:
1. Requests full snapshot from trusted peer
2. Validates checkpoint chain integrity (hash linkage, hash recomputation)
3. Verifies BLS signatures with stake-weighted verification (if genesis validators configured)
4. Atomically replaces state (accounts, validators, checkpoints, DAG)

### Trust Bootstrap System
Production nodes use a hybrid trust model combining genesis validators and stake-weighted verification:

**Environment Variables:**
- `GENESIS_VALIDATORS`: Semicolon-separated list of trusted genesis validators. Format: `address1:bls_pubkey_hex;address2:bls_pubkey_hex`
- `CHECKPOINT_QUORUM_THRESHOLD`: Fraction of stake required for quorum (default: 0.67 = 67%)
- `TRUST_CHECKPOINT_HASH`: Weak subjectivity checkpoint hash for fast bootstrap

**Trust Model:**
1. **Genesis validators** are hardcoded trusted roots with 1000 stake each
2. **On-chain validators** contribute stake from their registered stake amount (requires `bls_public_key` set)
3. **Checkpoint verification** requires BLS signatures from validators representing >67% of total stake
4. **Weak subjectivity** allows specifying a known-good checkpoint hash to skip full verification

**Testnet Mode:** If no genesis validators are configured, nodes run in testnet mode with BLS format validation only (signatures not cryptographically verified).

### Test Coverage
The Rust node includes comprehensive test coverage (99 tests total):

**Unit Tests (79 tests in rinku-node/src/):**
- `gossip.rs`: BoundedHashSet tests (8) - FIFO eviction, capacity limits, duplicate handling
- `trust.rs`: TrustVerifier tests (11) - genesis/on-chain validator lookup, checkpoint chain verification, signature requirements
- `checkpoint.rs`: Checkpoint hash computation tests (7) - determinism, field sensitivity, hex encoding
- `persistence.rs`, `slashing.rs`, `state_trie.rs`, `versioning.rs`, `validator.rs`: Component-specific tests

**Multi-Node Integration Tests (20 tests in rinku-node/tests/multi_node.rs):**
- `fork_detection_tests`: Identical nodes no-fork, different transactions fork detection, previous_hash mismatch detection, fork recovery threshold triggering
- `snapshot_sync_tests`: Account preservation, balance restoration, sync without full history, checkpoint chain preservation
- `checkpoint_adoption_tests`: Matching transaction adoption, different transaction rejection, chain linkage validation, hash recomputation
- `fork_resolution_tests`: DAG structure/tips, conflicting nonce detection, weight comparison logic
- `delta_sync_tests`: Missing transaction fetching, paginated sync
- `network_partition_tests`: Divergent chain creation, partition heal fork resolution

Run all tests: `cd packages/rinku-node && cargo test`

### Fly.io Deployment
The Rust node can be deployed to Fly.io for production use:
- `fly.toml`: Fly.io app configuration (auto-scaling, health checks, persistent storage)
- `Dockerfile.fly`: Multi-stage Rust build for minimal image size (~80MB)
- `.dockerignore`: Excludes TypeScript packages for faster builds

Deploy with:
```bash
fly launch --dockerfile Dockerfile.fly  # First time
fly deploy                              # Subsequent deploys
```

## External Dependencies
- **Cargo workspaces:** Rust monorepo management.
- **npm workspaces:** TypeScript monorepo management.
- **React:** Frontend development for the explorer.
- **Vite:** Frontend tooling.
- **Express:** Backend API servers for TypeScript faucet.
- **Web Crypto API:** Cryptographic operations (ECDSA P-256, SHA-256).
- **@noble/curves:** BLS12-381 signature aggregation.
- **@noble/hashes:** SHA-256 hashing.
- **pako:** DEFLATE compression.
- **vitest:** Testing framework.
- **circomlib:** ZK circuit library.
- **snarkjs:** Groth16 ZK-SNARK prover and verifier.
- **circomlibjs:** JavaScript implementation of Poseidon hash.
- **Rust Dependencies:** `p256`, `sha2`, `petgraph`, `tokio`, `axum`, `serde`, `serde_json`, `flate2`, `sled`, `tower-http`, `tracing`.