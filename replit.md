# Rinku - URL-Native Distributed Ledger

## Overview
Rinku is a URL-native distributed ledger leveraging DAG-based consensus and weight-based Sybil resistance. Its core innovation is embedding the entire ledger state within cryptographically-linked URLs, enabling a self-crawlable and trustlessly verifiable chain without traditional node infrastructure. The project aims to deliver a highly decentralized, robust, and verifiable distributed ledger with a hard-capped token supply, dynamic gas fees, and optional ZK privacy.

## User Preferences
I want to work iteratively. Please ask before making major changes. I prefer detailed explanations for complex features.

## System Architecture

### UI/UX Decisions
- **Explorer:** A React-based block explorer visualizes the DAG, accounts, faucet, contracts, and staking.
- **In-Browser Wallet:** A built-in wallet uses the Web Crypto API for ECDSA P-256 keypair generation and client-side transaction signing.

### System Design Choices
- **URL-Native Ledger:** The ledger state is embedded in cryptographically linked URLs, allowing self-crawlable and verifiable transactions. Transactions are base64url-encoded, deflated JSON objects within URLs.
- **DAG-Based Consensus:** Transactions reference global DAG "tips," resolving conflicts via cumulative weight for Sybil resistance.
- **Trustless Verification:** Finality proofs, embedded in URL query parameters, enable complete cryptographic validation of transactions and ledger state directly from URLs.
- **Smart Contracts:** Contract code and state are URL-encoded, with calls embedded within transactions.
- **Reward and Staking System:** Supports Tip, Stake, and Witness Rewards, with a staking mechanism for validators including slashing and an unbonding queue.
- **Dynamic Gas Fee Model (EIP-1559 Style):** Utilizes a utilization-based pricing model with an adaptive fee split.
- **Tokenomics System:** Features a hard-capped supply (30M RKU), genesis allocation, checkpoint-based emission with 18-month halving epochs, and WPoS reward distribution.
- **Multi-Node Networking:** Employs a gossip protocol for peer discovery and a snapshot-based sync protocol for efficient state synchronization.
- **Performance Optimizations:** Includes in-memory DAG pruning, snapshot optimizations, checkpoint-bounded self-crawlable URLs, per-transaction finality, self-contained Merkle proofs, batched operations, parallel signature verification, and a batch transaction API.
- **BLS Signature Aggregation:** Uses BLS12-381 for compact checkpoint validator signatures.
- **Compact Proof Format:** Self-contained finality proofs are designed for size efficiency, using DEFLATE compression and base64url encoding.
- **Self-Contained Proof System (MerkleSumTree Multi-Proof):** Provides fully offline-verifiable transaction proofs with chain identity binding.
- **ZK Privacy Layer:** Offers optional privacy-preserving proofs using Groth16 ZK-SNARKs for transactions.
- **Protocol Versioning & Upgrades:** Implements semantic versioning, feature flags, upgrade proposals, and peer compatibility checks.
- **Scalable State Architecture:** Utilizes persistent storage with `redb`, JSON serialization with `serde_json`, checkpoint-bounded DAG pruning, and a 256-level Sparse Merkle Trie for verifiable state roots. Account-based sharding is designed for large-scale account management.

### Technical Implementations
- **Core Library:** A shared library containing types, cryptography, encoding, Merkle trees, DAG structures, and weight calculation.
- **Fork Remediation Service:** Detects double-spends and handles non-conflicting DAG tips as healthy concurrent branches.
- **P2P Networking Architecture:** Implements a production-grade P2P layer using `libp2p` with TCP transport, Noise encryption, Yamux multiplexing, mDNS, and Bootstrap peer configuration.
- **Messaging Layer:** Uses GossipSub for pub/sub and CBOR-serialized request/response for sync operations.
- **DoS Protection:** Includes connection limits, rate limiting via token bucket algorithm, and peer banning.
- **Verified Sync with Merkle Proofs:** Incorporates snapshot verification, account proofs, and a `SyncVerifier` for tamper detection.
- **Slashing-Consensus Integration:** `ConsensusService` integrates with `SlashingService` for unified slashing, double-sign detection, and automatic slashing based on `LIVENESS_MISS_THRESHOLD`.
- **DAG Pruning Scheduling:** `CheckpointService` integrates `DagPruningService`, with `prune_dag()` called via `NodeState.storage()` getter.
- **Multi-Validator Quorum Collection:** `collect_validator_quorum()` gathers BLS signatures from peers with fallback to single-validator mode, validating votes against local validator registry and stake weights.
- **Leader Election:** Checkpoint creation uses stake-weighted leader election based on VRF-style randomness derived from the previous checkpoint hash.

## External Dependencies
- **Monorepo Management:** Cargo workspaces (Rust), npm workspaces (TypeScript).
- **Frontend:** React, Vite.
- **Backend:** Rust node (Axum) for consensus/API, Express (for TypeScript faucet).
- **Cryptography:** Web Crypto API, `@noble/curves` (BLS12-381), `@noble/hashes` (SHA-256).
- **Compression:** `pako` (DEFLATE).
- **Testing:** `vitest`.
- **ZK-SNARKs:** `circomlib`, `snarkjs`, `circomlibjs`.
- **Rust Libraries:** `p256`, `sha2`, `petgraph`, `tokio`, `axum`, `serde`, `serde_json`, `flate2`, `redb`, `tower-http`, `tracing`, `libp2p` (gossipsub, request-response, mDNS, identify).