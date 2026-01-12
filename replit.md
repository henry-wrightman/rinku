# Rinku - URL-Native Distributed Ledger

## Overview
Rinku (Japanese for "link") is a URL-native distributed ledger project that utilizes DAG-based consensus and weight-based Sybil resistance. Its key innovation is enabling the entire ledger state to exist as cryptographically-linked URLs, allowing for a self-crawlable and verifiable chain without traditional node infrastructure. The project aims to create a highly decentralized, robust, and trustlessly verifiable distributed ledger, pioneering a new approach to blockchain interaction and verification. It features a hard-capped token supply, dynamic gas fees, and optional ZK privacy.

## User Preferences
I want to work iteratively. Please ask before making major changes. I prefer detailed explanations for complex features.

## System Architecture

### UI/UX Decisions
- **Explorer:** A React-based block explorer for visualizing the DAG, accounts, faucet, contracts, and staking.

### System Design Choices
- **URL-Native Ledger:** The entire ledger state is embedded in cryptographically linked URLs, making the chain self-crawlable and verifiable. Transactions are base64url-encoded, deflated JSON objects directly in URLs.
- **DAG-Based Consensus:** Accounts maintain micro-chains, with transactions referencing multiple prior "tips." Conflicts are resolved by cumulative weight, providing Sybil resistance.
- **Trustless Verification:** Finality proofs, embedded as URL query parameters, allow complete cryptographic validation of transactions and ledger state directly from URLs.
- **Smart Contracts:** Contract code and state are URL-encoded, with calls embedded within transactions.
- **Reward and Staking System:** Supports Tip, Stake, and Witness Rewards, with a staking mechanism for validators including slashing penalties and an unbonding queue.
- **Dynamic Gas Fee Model (EIP-1559 Style):** Utilization-based pricing adjusts based on transaction volume, with an adaptive fee split (70%+ to validators, up to 30% burned).
- **Tokenomics System:** Hard-capped supply (30M RKU), genesis allocation, checkpoint-based emission with 18-month halving epochs, and WPoS reward distribution.
- **Multi-Node Networking:** Gossip protocol for peer discovery and a peer sync protocol for state synchronization.
- **Performance Optimizations:** Includes in-memory DAG pruning, snapshot optimizations, checkpoint-bounded self-crawlable URLs, per-transaction finality, self-contained Merkle proofs, batched operations, parallel signature verification, and batch transaction API.
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
- **Testnet Tooling (scripts/):**
  - `generate-proofs.ts`: Proof generation and verification pipeline using core functions (ECDSA via `verify`, Merkle via `verifyMerkleProof`, BLS via `verifySelfContainedProof`)
  - `proof-size-benchmark.ts`: Measures actual node-generated proof URL sizes for QR code compatibility
  - `validate-multi-node.ts`: Cross-node consensus validation (Merkle roots, checkpoints, transaction sets)
  - `local-testnet.sh`: Multi-node orchestration script
  - `TESTNET_SETUP.md`: 3-node testnet setup documentation

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