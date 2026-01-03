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
- **Validator Key Management:** AES-256-GCM encrypted key storage with scrypt key derivation (N=16384, r=8, p=1). Keys persist across restarts when password is consistent. Production requires `VALIDATOR_KEY_PASSWORD` environment variable; development uses a consistent default password.
- **Proof Slashing Service:** Validates Profile B proofs cryptographically, detecting duplicate signatures, weight mismatches, and forged validators. Triggers automatic slashing for invalid proofs (20%), invalid witnesses (15%), and receipt tampering (25%).
- **Gossip Protocol:** Real-time peer-to-peer transaction propagation, tip announcements, checkpoint signature aggregation, and validator set synchronization. Broadcasts transactions when submitted and periodically syncs with online peers.
- **Fork Remediation Service:** Nonce-based double-spend detection, weight-based conflict resolution with cumulative descendant weights, and branch pruning for losing forks. Automatically resolves forks when weight advantage exceeds 67%. Uses periodic summary logging (30s intervals) to reduce log verbosity.
- **BLS Signature Aggregation:** Uses BLS12-381 shortSignatures (48-byte G1 signatures, 96-byte G2 public keys) via @noble/curves for checkpoint validator signatures. Achieves 94.9% compression for 21 validators (1008B → 51B aggregated). Enables QR-compatible compact proofs.
- **Compact Proof Format (Profile B Compact):** Self-contained finality proofs that fit in QR v15 codes (~688 chars for 10-level Merkle + any validator count). Includes: version(1B) + txHash(32B) + txSig(64B ECDSA) + cpHeight(varint) + merkleProof(320B) + aggSig(48B BLS) + bitmap(3B) + valRoot(32B). Uses DEFLATE compression and base64url encoding.
- **Self-Contained Proof System (v5 - MerkleSumTree Multi-Proof):** Fully offline-verifiable transaction proofs with chain identity binding. Each proof contains: chainId (network identifier), transaction data, checkpoint header (txMerkleRoot, stateRoot, receiptRoot, tipCount), Merkle inclusion proof, BLS aggregated signature, MerkleSumTree multi-proof (batched signer membership proofs with shared siblings), and validatorSumTreeRoot (hash + totalWeight). Verification derives totalWeight from the MerkleSumTree root (cryptographically bound), recomputes signer weight from membership proofs, verifies 67% weight threshold, and confirms chainId matches expected network. Multi-proof optimization achieves 60-75% size reduction by sharing sibling nodes. Format: `rinku://sp/{base64url-deflate-packed}`. QR-compatible with packed encoding + multi-proof for committees N ≤ 21.

### Multi-Node Configuration
To run multiple nodes that can reconcile:
- Set `NODE_PEERS` to comma-separated peer URLs
- Set `NODE_ID` to a unique identifier per node
- Set `SELF_URL` to the node's public URL
- Set `GOSSIP_ENABLED=true` (default)
- Set `GOSSIP_INTERVAL_MS` for gossip frequency (default: 200ms)
- Set `CRYPTO_WORKERS` for parallel signature verification threads (default: CPU cores - 1)

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