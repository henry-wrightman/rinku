# Rinku - URL-Native Distributed Ledger

### Overview
Rinku is a URL-native distributed ledger that utilizes DAG-based consensus and weight-based Sybil resistance. Its key innovation lies in embedding the entire ledger state within cryptographically-linked URLs, enabling a self-crawlable and verifiable chain without relying on traditional node infrastructure. The project aims to provide a highly decentralized, robust, and trustlessly verifiable distributed ledger, pioneering a new approach to blockchain interaction and verification.

### User Preferences
I want to work iteratively. Please ask before making major changes. I prefer detailed explanations for complex features.

### System Architecture

**UI/UX Decisions:**
- **Explorer:** A React-based block explorer for visualizing the DAG, accounts, and interacting with the network.

**System Design Choices:**
- **URL-Native Ledger:** Ledger state is embedded in cryptographically linked URLs, making it self-crawlable and verifiable. Transactions are base64url-encoded, deflated JSON objects in URLs.
- **DAG-Based Consensus:** Accounts maintain micro-chains, resolving conflicts by cumulative weight, providing Sybil resistance.
- **Trustless Verification:** Finality proofs, embedded in URL query parameters, allow complete cryptographic validation directly from URLs.
- **Decentralization:** Consensus emerges from weighted votes among nodes, ensuring no single point of control.
- **Smart Contracts:** Contract code and state are URL-encoded, with calls embedded in transactions.
- **Reward and Staking System:** Supports Tip, Stake, and Witness Rewards, with a staking mechanism for validators.
- **Dynamic Gas Fee Model (EIP-1559 Style):** Utilization-based pricing that adjusts per checkpoint period.
- **Tokenomics System:** Features a hard-capped supply (30M RKU), genesis allocation, checkpoint-based emission with halving epochs, adaptive fee split, and WPoS reward distribution.
- **Multi-Node Networking:** Gossip protocol for peer discovery and a peer sync protocol for state synchronization.
- **Performance Optimizations:** Includes in-memory DAG pruning, snapshot optimizations, checkpoint-bounded self-crawlable URLs, per-transaction finality, self-contained Merkle proofs, batched operations, parallel signature verification, and a batch transaction API.
- **Finality Proofs:** Periodic checkpoints signed by staked validators enable trustless verification.
- **Self-Crawlable Bundles:** A `/txp/{payload}` URL format for bundles containing transaction ancestry back to the last finalized checkpoint.
- **Merkle Proofs:** Transactions include Merkle proofs for self-contained verification.
- **Validator Key Management:** AES-256-GCM encrypted key storage with scrypt key derivation.
- **API Rate Limiting:** Tiered rate limiting for different API endpoints.
- **Prometheus Metrics:** `/metrics` endpoint exposes standard Prometheus metrics for network monitoring.
- **Proof Slashing Service:** Validates proofs cryptographically and triggers automatic slashing for invalid proofs.
- **Gossip Protocol:** Real-time peer-to-peer transaction propagation, tip announcements, and checkpoint signature aggregation.
- **Fork Remediation Service:** Nonce-based double-spend detection, weight-based conflict resolution, and branch pruning for losing forks.
- **Protocol-Level Tip Consolidation:** Automatic DAG tip reduction using validator-created consolidation transactions.
- **BLS Signature Aggregation:** Uses BLS12-381 for compact checkpoint validator signatures, enabling significant compression.
- **Compact Proof Format (Profile B Compact):** Self-contained finality proofs designed to fit in QR codes.
- **Self-Contained Proof System (v5 - MerkleSumTree Multi-Proof):** Fully offline-verifiable transaction proofs with chain identity binding and multi-proof optimization for size reduction.
- **ZK Privacy Layer:** Optional privacy-preserving proofs using Groth16 ZK-SNARKs for `rinku://zk/{payload}` URLs, allowing transaction validity proof without revealing sensitive details.
- **Protocol Versioning & Upgrades:** Enables smooth network upgrades via semantic versioning, feature flags, upgrade proposals, and peer compatibility checks.

**Technical Implementations:**
- **Core Library:** Shared library for types, cryptography, encoding, Merkle trees, DAG structures, and weight calculation.
- **Rust Migration (In Progress):** Core library is being migrated to Rust for performance, targeting 1000+ TPS. Phase 1 (Rust core crate with crypto, Merkle, types) is complete.

### External Dependencies
- **npm workspaces:** Monorepo management.
- **React:** Frontend development.
- **Vite:** Frontend tooling.
- **Express:** Backend API servers.
- **Web Crypto API:** Cryptographic operations.
- **@noble/curves:** BLS12-381 signature aggregation.
- **@noble/hashes:** SHA-256 hashing.
- **pako:** DEFLATE compression.
- **vitest:** Testing framework.
- **circomlib:** ZK circuit library.
- **snarkjs:** Groth16 ZK-SNARK prover and verifier.
- **circomlibjs:** JavaScript implementation of Poseidon hash.
- **Rust (rinku-core-rs):** p256, sha2, serde, thiserror.