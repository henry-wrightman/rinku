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
- **Dynamic Gas Fee Model:** A demand-based pricing model with a portion of fees burned for deflation and the remainder distributed to active validators.
- **Tokenomics System:** Features a hard-capped supply, genesis allocation, checkpoint-based emission with a halving schedule, and a Weighted Proof-of-Stake (WPoS) reward distribution.
- **Multi-Node Networking:** A gossip protocol for peer discovery and a peer sync protocol for state synchronization.
- **Performance Optimizations:** Includes in-memory DAG pruning, snapshot optimizations using hash-based URLs, checkpoint-bounded self-crawlable URLs for efficient proof bundles, per-transaction finality, self-contained Merkle proofs, batched operations, parallel signature verification with worker threads, and batch transaction API.

**Technical Implementations:**
- **Core Library:** A shared library for types, cryptography (Web Crypto API), encoding, Merkle trees, DAG structures, and weight calculation.
- **Finality Proofs:** Periodic checkpoints signed by staked validators, enabling trustless verification.
- **Memory Management:** Configurable `MAX_DAG_NODES` and pruning mechanisms to manage in-memory transaction limits.
- **Witness Tracking:** TTL-based tracking to prevent duplicate rewards.
- **Self-Crawlable Bundles:** A new `/txp/{payload}` URL format for bundles containing transaction ancestry back to the last finalized checkpoint, maintaining the "link is the proof" property with bounded URL sizes.
- **Merkle Proofs:** Transactions include Merkle proofs for self-contained verification, validated against checkpoint-snapshotted transaction hashes.
- **Dynamic Gas Fees:** Demand-based pricing for transaction fees, with 50% burned and 50% distributed to validators.
- **Tokenomics:** Implements a fixed maximum supply, genesis allocation, emission schedule with halvings, and a reward distribution mechanism based on stake weight and account age. Includes slashing penalties for validator misconduct.
- **Finality Metrics System:** Tracks time-to-finality, pending transaction counts, and checkpoint latency to monitor network performance.
- **Validator Key Management:** AES-256-GCM encrypted key storage with scrypt key derivation (N=16384, r=8, p=1). Keys persist across restarts when password is consistent. Production requires `VALIDATOR_KEY_PASSWORD` environment variable; development uses a consistent default password.
- **Proof Slashing Service:** Validates Profile B proofs cryptographically, detecting duplicate signatures, weight mismatches, and forged validators. Triggers automatic slashing for invalid proofs (20%), invalid witnesses (15%), and receipt tampering (25%).
- **Gossip Protocol:** Real-time peer-to-peer transaction propagation, tip announcements, checkpoint signature aggregation, and validator set synchronization. Broadcasts transactions when submitted and periodically syncs with online peers.
- **Fork Remediation Service:** Nonce-based double-spend detection, weight-based conflict resolution with cumulative descendant weights, and branch pruning for losing forks. Automatically resolves forks when weight advantage exceeds 67%. Uses periodic summary logging (30s intervals) to reduce log verbosity.
- **BLS Signature Aggregation:** Uses BLS12-381 shortSignatures (48-byte G1 signatures, 96-byte G2 public keys) via @noble/curves for checkpoint validator signatures. Achieves 94.9% compression for 21 validators (1008B → 51B aggregated). Enables QR-compatible compact proofs.
- **Compact Proof Format (Profile B Compact):** Self-contained finality proofs that fit in QR v15 codes (~688 chars for 10-level Merkle + any validator count). Includes: version(1B) + txHash(32B) + txSig(64B ECDSA) + cpHeight(varint) + merkleProof(320B) + aggSig(48B BLS) + bitmap(3B) + valRoot(32B). Uses DEFLATE compression and base64url encoding.
- **Self-Contained Proof System (v3):** Fully offline-verifiable transaction proofs with embedded validator witnesses. Each proof contains: transaction data, checkpoint header (txMerkleRoot, stateRoot, receiptRoot, totalWeight, tipCount), Merkle inclusion proof, BLS aggregated signature, and full ValidatorWitness array with (index, address, blsPublicKey, weight). Verification computes validatorSetRoot from witnesses to prevent tampering, recomputes signer weight from embedded data (not trusting claimed values), and verifies 67% weight threshold. Format: `rinku://sp/{base64url-deflate-json}`. QR-compatible with DEFLATE compression.

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