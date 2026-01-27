# Rinku - URL-Native Distributed Ledger

## Overview
Rinku is a URL-native distributed ledger leveraging DAG-based consensus and weight-based Sybil resistance. Its core innovation lies in embedding the entire ledger state within cryptographically-linked URLs, enabling a self-crawlable and trustlessly verifiable chain without reliance on traditional node infrastructure. The project aims for a highly decentralized, robust, and verifiable distributed ledger featuring a hard-capped token supply, dynamic gas fees, and optional ZK privacy. Its vision is to unlock new paradigms for decentralized applications and provide a scalable, secure, and transparent financial backbone.

## User Preferences
I want to work iteratively. Please ask before making major changes. I prefer detailed explanations for complex features.

## System Architecture

### UI/UX Decisions
- **Explorer:** A React-based block explorer provides visualizations of the DAG, accounts, faucet, contracts, and staking.
- **In-Browser Wallet:** Utilizes the Web Crypto API for ECDSA P-256 keypair generation and client-side transaction signing.

### System Design Choices
- **URL-Native Ledger:** The entire ledger state is embedded and discoverable via cryptographically linked URLs.
- **DAG-Based Consensus:** Transactions reference global DAG "tips," employing cumulative weight for conflict resolution and Sybil resistance.
- **Trustless Verification:** Finality proofs embedded in URL query parameters facilitate direct cryptographic validation.
- **Smart Contracts:** Contract code and state are URL-encoded, with calls integrated within transactions.
- **Reward and Staking System:** Incorporates Tip, Stake, and Witness Rewards with a validator staking mechanism including slashing.
- **Dynamic Gas Fee Model:** An EIP-1559 style model with utilization-based pricing and adaptive fee splitting.
- **Tokenomics System:** Features a hard-capped supply (30M RKU), genesis allocation, checkpoint-based emission with halving epochs, and WPoS reward distribution.
- **Multi-Node Networking:** Employs a gossip protocol for peer discovery and snapshot-based synchronization.
- **Performance Optimizations:** Includes in-memory DAG pruning, snapshot optimizations, checkpoint-bounded self-crawlable URLs, per-transaction finality, self-contained Merkle proofs, batched operations, parallel signature verification, and a batch transaction API.
- **BLS Signature Aggregation:** Uses BLS12-381 for compact checkpoint validator signatures.
- **Compact Proof Format:** Self-contained finality proofs are designed for size efficiency using DEFLATE and base64url encoding.
- **Self-Contained Proof System (MerkleSumTree Multi-Proof):** Enables fully offline-verifiable transaction proofs.
- **ZK Privacy Layer:** Offers optional privacy-preserving proofs using Groth16 ZK-SNARKs.
- **Protocol Versioning & Upgrades:** Implements semantic versioning, feature flags, upgrade proposals, and peer compatibility checks.
- **Scalable State Architecture:** Features persistent storage with `redb`, JSON serialization, checkpoint-bounded DAG pruning, and a 256-level Sparse Merkle Trie for verifiable state roots. Account-based sharding is designed for large-scale account management.

### Technical Implementations
- **Core Library:** A shared library provides common types, cryptography, encoding, Merkle trees, and DAG structures.
- **P2P Networking Architecture:** Built using `libp2p` with TCP, Noise encryption, Yamux, mDNS, and Bootstrap for robust peer-to-peer communication.
- **Messaging Layer:** Utilizes GossipSub for pub/sub and CBOR-serialized request/response for sync operations.
- **DoS Protection:** Implements connection limits, rate limiting, and peer banning mechanisms.
- **API Backpressure Protection:** Rejects new transactions with HTTP 503 when DAG tips exceed a defined threshold.
- **Pending Nonce Queue:** Buffers future-nonce transactions for later processing, incorporating DDoS protection.
- **Transaction Memo & References:** Supports optional `memo` (max 256 bytes) and `references` (max 4 transaction hashes) fields.
- **DAG Pruning Scheduling:** `CheckpointService` integrates with `DagPruningService` for efficient DAG management.
- **Multi-Validator Quorum Collection:** Gathers BLS signatures from peers, validating votes against a local registry and stake weights.
- **Leader Election (Validator-Based):** Stake-weighted leader election based on VRF-style randomness from the previous checkpoint hash.
- **High-Volume Sync Optimization:** Delta sync batch size is increased to 1000 transactions per page for faster catch-up.
- **Sparse DAG Sampling (Phase 1 Scaling):** Transactions reference at most 16 tips to prevent tip explosion.
- **Shoal++ Anchor Transactions (Phase 2 Scaling):** Validators emit anchor transactions to create DAG convergence points.
- **Checkpoint Announcement Fast-Broadcast (Phase 3 Scaling):** Dedicated `GossipMessage::CheckpointAnnouncement` for immediate checkpoint propagation.
- **Propagation Grace Period:** A 5-second grace period (`PROPAGATION_GRACE_MS`) is added before including transactions in checkpoints to ensure network propagation.
- **Finalized Transaction Hashes in Checkpoints:** `CheckpointAnnouncement` now includes `finalized_tx_hashes` to explicitly communicate finalized transactions, preventing Merkle root mismatches.
- **Stale Nonce Quick-Reject Cache:** `GossipServiceInner` maintains a `stale_nonce_cache` to track minimum expected nonces, dropping transactions with lower nonces before full validation, significantly reducing CPU load during high transaction volume.
- **Bounded Pending Transaction Queue:** The `pending_txs` queue in `GossipServiceInner` is bounded to `PENDING_TXS_MAX` (10,000 transactions) to prevent memory exhaustion.
- **Increased Mempool Limits:** `MAX_NONCE_GAP`, `MAX_PENDING_PER_SENDER`, `MAX_PENDING_SENDERS`, and `MAX_GLOBAL_PENDING` limits are increased to support high-volume dapps.
- **Non-Blocking Propagation Architecture:** `propagate_pending_txs` is refactored into a spawned background task to prevent blocking the gossip round and ensure API responsiveness.
- **Optimized Delta Sync (`sync_from_peer_optimized`):** Starts syncing from an offset near `local_dag_size-500` to dramatically reduce bandwidth for catching up on recent transactions.
- **Dashboard Stats O(1) Fix:** `get_dashboard_stats()` now uses O(1) methods to prevent API lock contention and improve response times.
- **P2P Message Size Limit Increase:** `max_transmit_size` in gossipsub configuration is increased to 2MB to accommodate large checkpoint announcements.
- **Nonce Reconciliation After Sync:** Implemented in `apply_sync_snapshot_inner()` to reconcile account nonces after syncing, preventing "phantom nonces" and "stale" transaction rejections.
- **P2P Propagation Batch Limit:** `propagate_pending_txs_background()` now limits P2P broadcasts to `MAX_PROPAGATION_BATCH` (100) transactions per cycle, re-queuing overflow for subsequent cycles. This prevents gossipsub layer overload that caused validator crashes during stress tests (783 txs in one batch caused OOM).
- **Rate Limit Without Banning:** Rate limit violations no longer trigger peer score penalties or bans. Messages are simply dropped, preserving consensus participation during high-load scenarios. This fixes false-positive validator bans during stress tests.
- **Checkpoints With Finalized Tx Hashes:** Checkpoints are now created to include finalized_tx_hashes to match the leader's original list from which the merkle tree was built from

## External Dependencies
- **Monorepo Management:** Cargo workspaces (Rust), npm workspaces (TypeScript).
- **Frontend:** React, Vite.
- **Backend:** Rust node (Axum), Express (TypeScript faucet).
- **Cryptography:** Web Crypto API, `@noble/curves`, `@noble/hashes`.
- **Compression:** `pako`.
- **Testing:** `vitest`.
- **ZK-SNARKs:** `circomlib`, `snarkjs`, `circomlibjs`.
- **Rust Libraries:** `p256`, `sha2`, `petgraph`, `tokio`, `axum`, `serde`, `serde_json`, `flate2`, `redb`, `tower-http`, `tracing`, `libp2p`.