# Rinku - URL-Native Distributed Ledger

### Overview
Rinku (Japanese for "link") is a URL-native distributed ledger leveraging DAG-based consensus and weight-based Sybil resistance. Its core innovation allows the entire ledger state to exist as cryptographically-linked URLs, enabling a self-crawlable and verifiable chain without traditional node infrastructure. The project aims to deliver a highly decentralized, robust, and trustlessly verifiable distributed ledger, fostering a new paradigm for blockchain interaction and verification.

### User Preferences
I want to work iteratively. Please ask before making major changes. I prefer detailed explanations for complex features.

### System Architecture

**UI/UX Decisions:**
- **Explorer:** A React-based block explorer for visualizing the DAG, accounts, and interacting with the faucet, contracts, and staking.

**System Design Choices:**
- **URL-Native Ledger:** The entire ledger state is embedded in cryptographically linked URLs, enabling a self-crawlable and verifiable chain without traditional node infrastructure. Transactions are base64url-encoded, deflated JSON objects embedded directly in URLs.
- **DAG-Based Consensus:** Accounts maintain micro-chains, with transactions referencing multiple prior "tips." Conflicts are resolved by cumulative weight, providing Sybil resistance based on account age and balance.
- **Trustless Verification:** Finality proofs, embedded as URL query parameters, allow for complete cryptographic validation of transactions and the ledger state directly from URLs, independent of a centralized node.
- **Decentralization:** Consensus emerges from weighted votes among nodes, ensuring no single point of control.
- **Smart Contracts:** Contract code and state are URL-encoded, with calls embedded within transactions.
- **Reward and Staking System:** Supports Tip, Stake, and Witness Rewards, alongside a staking mechanism for validators with slashing penalties and an unbonding queue.
- **Dynamic Gas Fee Model (EIP-1559 Style):** Utilization-based pricing that adjusts ±12.5% per checkpoint period based on txs vs target. Self-correcting to prevent runaway fee spikes. Adaptive fee split: 70%+ to validators (floor), up to 30% burned as supply grows toward 50% target.
- **Tokenomics System:** Features a hard-capped supply (30M RKU), genesis allocation (6M RKU), checkpoint-based emission with 18-month halving epochs, adaptive fee split (70%+ validator floor), and WPoS reward distribution with anti-gaming measures (min bond for age weight, decay for missed checkpoints).
- **Multi-Node Networking:** A gossip protocol for peer discovery and a peer sync protocol for state synchronization.
- **Performance Optimizations:** Includes in-memory DAG pruning, snapshot optimizations using hash-based URLs, checkpoint-bounded self-crawlable URLs for efficient proof bundles, per-transaction finality, self-contained Merkle proofs, batched operations, parallel signature verification with worker threads, and batch transaction API.

**Technical Implementations:**
- **Core Library:** A shared library for types, cryptography (Web Crypto API), encoding, Merkle trees, DAG structures, and weight calculation.
- **Finality Proofs:** Periodic checkpoints signed by staked validators, enabling trustless verification.
- **Memory Management:** Configurable `MAX_DAG_NODES` and pruning mechanisms to manage in-memory transaction limits.
- **Witness Tracking:** TTL-based tracking to prevent duplicate rewards.
- **Self-Crawlable Bundles:** A new `/txp/{payload}` URL format for bundles containing transaction ancestry back to the last finalized checkpoint, maintaining the "link is the proof" property with bounded URL sizes.
- **Merkle Proofs:** Transactions include Merkle proofs for self-contained verification, validated against checkpoint-snapshotted transaction hashes.
- **Dynamic Gas Fees (EIP-1559 Style):** Utilization-based pricing adjusts ±12.5% per 15s period. Target: 15 txs/period. Max fee: 10 RKU. Self-correcting (no runaway feedback loops). Adaptive fee split: 70%+ to validators (floor), up to 30% burned as supply grows toward 50% target.
- **Tokenomics:** Implements fixed maximum supply (30M RKU), genesis allocation (6M RKU), 18-month halving epochs (~3.15M checkpoints), adaptive fee/burn split (70%+ validator floor), and WPoS reward distribution with anti-gaming (min bond for age weight, 10% decay per missed checkpoint). Emission: 3.934 RKU/checkpoint epoch 0, halving to 0.123 RKU floor. Total emission ~24M RKU over ~7.5 years. Includes slashing penalties for validator misconduct.
- **Finality Metrics System:** Tracks time-to-finality, pending transaction counts, and checkpoint latency to monitor network performance.
- **Validator Key Management:** AES-256-GCM encrypted key storage with scrypt key derivation (N=16384, r=8, p=1). Keys persist across restarts when password is consistent. Production requires `VALIDATOR_KEY_PASSWORD` or `VALIDATOR_KEY_PASSWORD_FILE` environment variable; development uses a consistent default password. Supports encrypted key backup/restore via `exportEncryptedBackup()` and `importEncryptedBackup()` methods.
- **API Rate Limiting:** Tiered rate limiting using express-rate-limit: TX endpoints (30 req/min), contract endpoints (20 req/min), general endpoints (100 req/min). Configurable via `RATE_LIMIT_WINDOW_MS`, `RATE_LIMIT_TX_MAX`, `RATE_LIMIT_CONTRACT_MAX`, `RATE_LIMIT_GENERAL_MAX`.
- **Prometheus Metrics:** `/metrics` endpoint exposes standard Prometheus metrics: DAG stats (nodes, tips), checkpoint height, gas price, validator count, total stake, supply, tx submitted/rejected counters. Compatible with Grafana dashboards.
- **Proof Slashing Service:** Validates Profile B proofs cryptographically, detecting duplicate signatures, weight mismatches, and forged validators. Triggers automatic slashing for invalid proofs (20%), invalid witnesses (15%), and receipt tampering (25%).
- **Gossip Protocol:** Real-time peer-to-peer transaction propagation, tip announcements, checkpoint signature aggregation, and validator set synchronization. Broadcasts transactions when submitted and periodically syncs with online peers.
- **Fork Remediation Service:** Nonce-based double-spend detection, weight-based conflict resolution with cumulative descendant weights, and branch pruning for losing forks. Automatically resolves forks when weight advantage exceeds 67%. Uses periodic summary logging (30s intervals) to reduce log verbosity. Queue-based fork detection limits to maxTipsForFullScan (20) tips per interval, cycling through all tips over multiple intervals to prevent O(n²) explosion. MAX_TIPS env var (default: 15) provides operational monitoring. Memory-bounded: fork events capped at 1000 entries, stale forks pruned after 2 minutes, fork detection skipped when tips > 50 to prevent CPU spikes.
- **BLS Signature Aggregation:** Uses BLS12-381 shortSignatures (48-byte G1 signatures, 96-byte G2 public keys) via @noble/curves for checkpoint validator signatures. Achieves 94.9% compression for 21 validators (1008B → 51B aggregated). Enables QR-compatible compact proofs.
- **Compact Proof Format (Profile B Compact):** Self-contained finality proofs that fit in QR v15 codes (~688 chars for 10-level Merkle + any validator count). Includes: version(1B) + txHash(32B) + txSig(64B ECDSA) + cpHeight(varint) + merkleProof(320B) + aggSig(48B BLS) + bitmap(3B) + valRoot(32B). Uses DEFLATE compression and base64url encoding.
- **Self-Contained Proof System (v5 - MerkleSumTree Multi-Proof):** Fully offline-verifiable transaction proofs with chain identity binding. Each proof contains: chainId (network identifier), transaction data, checkpoint header (txMerkleRoot, stateRoot, receiptRoot, tipCount), Merkle inclusion proof, BLS aggregated signature, MerkleSumTree multi-proof (batched signer membership proofs with shared siblings), and validatorSumTreeRoot (hash + totalWeight). Verification derives totalWeight from the MerkleSumTree root (cryptographically bound), recomputes signer weight from membership proofs, verifies 67% weight threshold, and confirms chainId matches expected network. Multi-proof optimization achieves 60-75% size reduction by sharing sibling nodes. Format: `rinku://sp/{base64url-deflate-packed}`. QR-compatible with packed encoding + multi-proof for committees N ≤ 21.
- **ZK Privacy Layer:** Optional privacy-preserving proofs using Groth16 ZK-SNARKs. Enables `rinku://zk/{payload}` URLs that prove transaction validity without revealing sender, recipient, or amount. Uses Poseidon hashing (ZK-friendly), Pedersen commitments for hidden amounts, EdDSA-Poseidon signatures (BabyJubJub curve via circomlibjs), and nullifiers to prevent double-claims. Base chain stays transparent; privacy is opt-in at the proof layer. ~500 char QR-compatible proofs with 10ms verification. Implementation: `packages/zk/` workspace with Circom circuits (10-level Merkle, ~10.5k constraints) and TypeScript infrastructure. Artifacts: WASM (3.14MB), zkey (6.71MB), vkey (3.39KB). Compilation uses Powers of Tau ceremony with crypto.randomBytes entropy. Node API: `/api/zk/status`, `/api/zk/witness/:txHash`, `/api/zk/verify`, `/api/zk/prove` (full proof generation). Explorer: ZK Privacy tab for witness generation, proof generation, and verification. 24 tests pass including end-to-end proof generation (~2.5s). See Appendix H in WHITEPAPER.md for full specification.
- **Protocol Versioning & Upgrades:** Enables smooth network upgrades without hard forks. Features: semantic versioning (major.minor.patch), feature flags with activation thresholds, upgrade proposals with validator signaling, and peer compatibility checks. API endpoints: `/api/version` (current version info), `/api/version/features` (active features), `/api/version/proposals` (pending upgrades), `/api/version/compatibility/:version` (peer compatibility check). Validators signal version support in checkpoint signatures. Features activate when signaling weight exceeds threshold (default 75%). Known features: bls-aggregation, zk-privacy, dynamic-gas, merkle-sum-tree, contract-receipts, adaptive-fee-burn.

### Node TUI Dashboard
Interactive terminal dashboard for node operators. Enable with `NODE_TUI=true` environment variable.
- **Dashboard View:** Real-time stats for DAG, consensus, system resources, and network
- **Key Bindings:** `L` (logs), `P` (peers), `D` (DAG details), `S` (system specs), `T` (thread config), `Q` (quit), `ESC` (back)
- **Thread Configuration:** Use ↑/↓ arrows in thread view to dynamically resize the crypto worker pool
- **Telemetry:** CPU usage, memory (heap/RSS), network I/O rates, disk usage

### Multi-Node Configuration
To run multiple nodes that can reconcile:
- Set `NODE_PEERS` to comma-separated peer URLs
- Set `NODE_ID` to a unique identifier per node
- Set `SELF_URL` to the node's public URL
- Set `GOSSIP_ENABLED=true` (default)
- Set `GOSSIP_INTERVAL_MS` for gossip frequency (default: 200ms)
- Set `CRYPTO_WORKERS` for parallel signature verification threads (default: CPU cores - 1)
- Set `NODE_TUI=true` to enable interactive terminal dashboard
- Set `RATE_LIMIT_TX_MAX` for transaction rate limit (default: 30/min)
- Set `RATE_LIMIT_CONTRACT_MAX` for contract rate limit (default: 20/min)
- Set `RATE_LIMIT_GENERAL_MAX` for general rate limit (default: 100/min)

### Stress Testing
Run `npm run stress-test` to execute the stress test suite. Tests include:
- Prometheus metrics endpoint validation
- Rate limiting verification
- Snapshot persistence check
- Tip consolidation monitoring
- Checkpoint progression tracking

### External Dependencies
- **npm workspaces:** Monorepo management.
- **React:** Frontend development for the explorer.
- **Vite:** Fast frontend tooling.
- **Express:** Backend API servers for nodes and faucet.
- **Web Crypto API:** Cryptographic operations (ECDSA P-256 signatures, SHA-256 hashing).
- **@noble/curves:** BLS12-381 signature aggregation for compact checkpoint proofs.
- **@noble/hashes:** SHA-256 hashing for BLS key fingerprints.
- **Whitepaper:** See WHITEPAPER.md for complete technical specification.
- **pako:** DEFLATE compression for URL payloads.
- **vitest:** Testing framework.
- **circomlib:** ZK circuit library for Poseidon, EdDSA, and Merkle proof circuits.
- **snarkjs:** Groth16 ZK-SNARK prover and verifier for TypeScript/JavaScript.
- **circomlibjs:** JavaScript implementation of Poseidon hash for off-chain computation.