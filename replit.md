# Rinku - URL-Native Distributed Ledger

### Overview
Rinku (Japanese for "link") is a URL-native distributed ledger project leveraging DAG-based consensus and weight-based Sybil resistance. Its core innovation allows the entire ledger state to exist as cryptographically-linked URLs, enabling a self-crawlable and verifiable chain without traditional node infrastructure. The project aims to deliver a highly decentralized, robust, and trustlessly verifiable distributed ledger, fostering a new paradigm for blockchain interaction and verification. It features a hard-capped token supply, dynamic gas fees, and optional ZK privacy.

### User Preferences
I want to work iteratively. Please ask before making major changes. I prefer detailed explanations for complex features.

### System Architecture

**UI/UX Decisions:**
- **Explorer:** A React-based block explorer for visualizing the DAG, accounts, faucet, contracts, and staking.

**System Design Choices:**
- **URL-Native Ledger:** The entire ledger state is embedded in cryptographically linked URLs, enabling a self-crawlable and verifiable chain. Transactions are base64url-encoded, deflated JSON objects directly in URLs.
- **DAG-Based Consensus:** Accounts maintain micro-chains, with transactions referencing multiple prior "tips." Conflicts are resolved by cumulative weight, providing Sybil resistance.
- **Trustless Verification:** Finality proofs, embedded as URL query parameters, allow complete cryptographic validation of transactions and ledger state directly from URLs.
- **Decentralization:** Consensus emerges from weighted votes among nodes.
- **Smart Contracts:** Contract code and state are URL-encoded, with calls embedded within transactions.
- **Reward and Staking System:** Supports Tip, Stake, and Witness Rewards, with a staking mechanism for validators including slashing penalties and an unbonding queue.
- **Dynamic Gas Fee Model (EIP-1559 Style):** Utilization-based pricing adjusts based on transaction volume, with an adaptive fee split (70%+ to validators, up to 30% burned).
- **Tokenomics System:** Hard-capped supply (30M RKU), genesis allocation, checkpoint-based emission with 18-month halving epochs, and WPoS reward distribution with anti-gaming measures.
- **Multi-Node Networking:** Gossip protocol for peer discovery and a peer sync protocol for state synchronization.
- **Performance Optimizations:** Includes in-memory DAG pruning, snapshot optimizations, checkpoint-bounded self-crawlable URLs, per-transaction finality, self-contained Merkle proofs, batched operations, parallel signature verification, and batch transaction API.
- **Protocol-Level Tip Consolidation:** Automatic DAG tip reduction via validator-created zero-fee consolidation transactions.
- **BLS Signature Aggregation:** Uses BLS12-381 for compact checkpoint validator signatures.
- **Compact Proof Format (Profile B Compact):** Self-contained finality proofs designed to fit in QR codes, utilizing DEFLATE compression and base64url encoding.
- **Self-Contained Proof System (v5 - MerkleSumTree Multi-Proof):** Fully offline-verifiable transaction proofs with chain identity binding, optimized for size reduction via multi-proofs.
- **ZK Privacy Layer:** Optional privacy-preserving proofs using Groth16 ZK-SNARKs, enabling `rinku://zk/{payload}` URLs for transactions without revealing sender, recipient, or amount.
- **Protocol Versioning & Upgrades:** Semantic versioning, feature flags with activation thresholds, upgrade proposals with validator signaling, and peer compatibility checks for smooth network upgrades.

**Technical Implementations:**
- **Core Library:** Shared library for types, cryptography (Web Crypto API), encoding, Merkle trees, DAG structures, and weight calculation.
- **Finality Proofs:** Periodic checkpoints signed by staked validators.
- **Memory Management:** Configurable DAG node limits and pruning mechanisms.
- **Self-Crawlable Bundles:** `/txp/{payload}` URL format for transaction ancestry.
- **Merkle Proofs:** Transactions include Merkle proofs for self-contained verification.
- **Fork Remediation Service:** Nonce-based double-spend detection, weight-based conflict resolution, and branch pruning for losing forks.
- **Node TUI Dashboard:** Interactive terminal dashboard for node operators, providing real-time stats and configuration.
- **Validator Key Management:** AES-256-GCM encrypted key storage with scrypt key derivation.
- **API Rate Limiting:** Tiered rate limiting for TX, contract, and general endpoints.
- **Prometheus Metrics:** `/metrics` endpoint for network monitoring.
- **Proof Slashing Service:** Validates Profile B proofs and triggers automatic slashing for invalid proofs.

### External Dependencies
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

### Recent Changes (January 2026)
- **Smart Contracts Execution Engine (Rust):** Implemented ContractService with GasSchedule, GasMeter, state diff computation, and support for init/transfer/mint/burn/get_balance entrypoints (9 tests passing)
- **ZK Privacy Layer (Rust):** Poseidon hash implementation with arkworks (ark-bn254, ark-groth16), Groth16 prover/verifier, NullifierRegistry with sled persistence, and ZK URL encoding/decoding (7 tests passing)
- **Fork Remediation Service (Rust):** Branch pruning wired to DAG operations with cumulative weight calculation (WEIGHT_THRESHOLD=1.5), double-spend detection via nonce indexing, and fork resolution with winner/loser tracking
- **Gossip Protocol (Rust):** Transaction propagation infrastructure, peer discovery, sync requests, conflict resolution broadcasting, and health monitoring with Arc<RwLock> thread-safe peer management (stubbed for libp2p integration)
- **BLS12-381 Cryptography Module:** Added complete BLS signature support using `blst` crate with key generation, signing, aggregation, verification, and signer bitmaps (4 tests passing)
- **Self-Contained Proof System (Rust):** Implemented SelfContainedProof (Profile C) and CompactProof (Profile B) structures with DEFLATE compression, base64url encoding, and Merkle proof verification (2 tests passing)
- **Checkpoint BLS Signing:** CheckpointService now signs checkpoints with BLS12-381 signatures, generating aggregated signatures and signer bitmaps for finality proofs
- **Self-Provable Transaction Endpoint:** Added `/api/txp/:hash` endpoint returning finalized transactions with Merkle proofs, checkpoint data, and proof URLs for offline verification
- **Full Service Implementation:** Added RewardsService (staking/distribution), EmissionService (tokenomics/halving), and SlashingService (validator penalties) to Rust node
- **Genesis Initialization:** Rust node now seeds faucet account with 1M RKU and creates genesis transaction on first startup, matching TypeScript behavior
- **Extended API Endpoints:** Added `/api/tx/:hash`, `/api/staking`, `/api/tokenomics/supply`, `/api/rewards/config` for explorer compatibility
- **Version Endpoint Updated:** `/api/version` now returns `protocolVersion`, `nodeVersion`, `chainId`, `networkId`, and `features` array
- **Legacy API Compatibility:** Added `/api/tipUrls` endpoint and legacy `{ tx: {...} }` format support for faucet and activity-bot compatibility
- **Rust Node API Compatibility:** Updated all API response structures to use camelCase via serde rename_all, aligning with TypeScript explorer expectations
- **New API Endpoints:** Added `/api/dag`, `/api/dag/summary`, `/api/accounts`, `/api/stats/network`, `/api/gas/price`, `/api/gas/stats`, `/api/finality/metrics`, `/api/version`, `/api/tipUrls`, `/api/tokenomics/emission`, `/api/tokenomics/slashing`, `/api/checkpoints`, `/api/checkpoints/latest`, `/api/rewards/:address`, `/api/staking/:address`, `/api/fork/stats`, `/api/gossip/stats`, `/api/tip-consolidator/stats`
- **Merkle Tree Security Fix:** Fixed critical bug where invalid hex leaves silently substituted zeros; now properly validates and returns errors
- **Sled Persistence:** Integrated sled database for state snapshots with automatic recovery on startup
- **Background Services:** Wired checkpoint (15s), gossip (200ms), fork remediation, and tip consolidation services into main event loop
- **Service Integration:** EmissionService, SlashingService, and RewardsService wired to NodeState with Arc<RwLock<>> for thread-safe API access. All tokenomics endpoints now return live service data.
- **Checkpoint Reward Processing:** CheckpointService now calls EmissionService.record_emission() and RewardsService.distribute_checkpoint_rewards() on each checkpoint, updating live emission totals and validator rewards.
- **Parent URL Normalization:** Transaction submission now correctly normalizes parent URLs (e.g., `rinku://tx/h/{hash}` → `{hash}`) for proper DAG parent resolution, enabling faucet/activity-bot compatibility.
- **MerkleSumTree (Rust):** Added buildMerkleSumTree, getMerkleSumProof, and verifyMerkleSumProof for BLS validator weight aggregation with deterministic SHA256 hashing (4 tests passing)
- **StateTrie Module (Rust):** Contract state management with Merkle proof generation/verification, contract isolation, and snapshot/restore functionality (6 tests passing)
- **Protocol Versioning Module (Rust):** FeatureFlags, UpgradeProposals, VersionCompatibility checks, and 5 known features with activation thresholds (5 tests passing)
- **Enhanced /api/version:** Now exposes 11 active features (dag-consensus, url-native, sled-persistence, finality-proofs, merkle-sum-tree, bls-aggregation, dynamic-gas, smart-contracts, tip-consolidation, fork-remediation, zk-privacy)
- **Fixed Finality Stats Tracking:** Added `get_finalized_stats()` method to NodeState, updated `/api/stats/network` and `/api/finality/metrics` to return real finalized/unfinalized counts and TPS
- **Fixed Transaction URL Format:** Changed DAG node URLs from `rinku://tx/{hash}` to `/tx/h/{hash}` for proper explorer React Router compatibility
- **Fixed /api/tx/:hash Response:** Updated to return `ts`, `tipUrls`, and `url` fields matching explorer expectations
- **Total Transaction Counter:** Added persistent `total_transactions` counter to NodeState that tracks all historical transactions (not pruned DAG nodes), ensuring accurate transaction count display in explorer
- **POST Staking/Contract Endpoints:** Added `POST /api/staking/stake`, `POST /api/contracts/deploy`, and `POST /api/contracts/:id/call` endpoints for activity-bot compatibility
- **Real Finality Metrics:** Fixed `/api/finality/metrics` to return real-time data: avg/median/p95 finality times from rolling 100-tx window, last checkpoint age, and checkpoints per minute. Previously hardcoded to 15s/20s defaults.
- **Finality Time Tracking:** Checkpoints now record finalization latency (time from tx creation to finalization) in a rolling VecDeque, enabling dynamic avg/p95 finality calculations
- **Timestamp Format Handling:** Fixed finality time calculation to handle both seconds and milliseconds timestamps correctly

### Development Notes
- Rust node runs on port 3001, TypeScript explorer on port 5000
- 60 Rust node tests + 30 core tests = 90 tests passing
- Hybrid architecture: Rust for consensus/validation, TypeScript for user-facing interfaces
- All Rust services match TypeScript behavior with proper serialization
- BLS crypto: `bls.rs` module with keypair generation, sign/verify, aggregate signatures/keys, and signer bitmaps
- Proofs: `proofs.rs` module with SelfContainedProof (verbose, Profile C), CompactProof (binary, Profile B), and MerkleSumTree for BLS validator weight aggregation
- StateTrie: `state_trie.rs` module for contract state management with Merkle proof generation/verification and snapshot/restore
- Versioning: `versioning.rs` module with FeatureFlags, UpgradeProposals, version compatibility checks, and 5 known features (bls-aggregation, zk-privacy, dynamic-gas, smart-contracts, tip-consolidation)
- Contracts: `contracts.rs` module with mock runtime (matching TypeScript createMockRuntime), GasSchedule, GasMeter, state diffs
- ZK Privacy: `zk.rs` module using `light-poseidon` crate (Veridise-audited) for circomlib-compatible Poseidon hash, Groth16 verification, NullifierRegistry with sled persistence
- Network: `network.rs` module with libp2p gossipsub protocol (SHA256 message IDs for deterministic deduplication), mDNS peer discovery, and NetworkHandle for async broadcasting
- Gossip: `gossip.rs` module for HTTP-based gossip fallback, with `network.rs` providing libp2p alternative
- Fork Remediation: `fork_remediation.rs` with cumulative weight calculation, branch pruning, nonce-based double-spend detection