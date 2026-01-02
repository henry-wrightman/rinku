# Rinku - URL-Native Distributed Ledger

### Overview
Rinku (Japanese for "link") is a URL-native distributed ledger featuring DAG-based consensus and weight-based Sybil resistance. Its core innovation is that the entire ledger state exists as cryptographically-linked URLs, enabling a self-crawlable and verifiable chain without traditional node infrastructure. The project aims to provide a highly decentralized and robust distributed ledger.

### User Preferences
I want to work iteratively. Please ask before making major changes. I prefer detailed explanations for complex features.

### System Architecture

**UI/UX Decisions:**
- **Explorer:** A React-based block explorer with a clean interface for visualizing the DAG, accounts, and interacting with faucet, contracts, and staking.

**Technical Implementations:**
- **Core Library:** Shared `packages/core` for types, cryptography (Web Crypto API), encoding, Merkle trees, DAG structures, and weight calculation.
- **Transaction URL Format:** Transactions are base64url-encoded, deflated JSON objects embedded directly in URLs. They include `from`, `to`, `amount`, `nonce`, `tipUrls` (full parent URLs), `sig`, and `ts`.
- **Self-Crawlable Ledger:** Each transaction URL embeds the full URLs of its parent transactions, allowing the entire ledger to be reconstructed and validated from any single tip URL without a dedicated node.
- **DAG-Based Ledger:** Accounts maintain micro-chains. Transactions reference multiple prior "tips," and conflicts are resolved by cumulative weight.
- **Weight Calculation:** Sybil resistance is achieved through a weight formula: `(account_age_days * 0.3) + (balance * 0.7)`.
- **Multi-Node Networking:**
    - **Gossip Protocol:** Nodes discover peers automatically by exchanging peer lists.
    - **Persistence:** State and DAG are persisted to JSON files for restart recovery.
    - **Peer Sync Protocol:** Endpoints (`/api/sync/status`, `/api/sync/transactions`, `/api/sync/peers`, etc.) facilitate node-to-node communication.
- **Smart Contracts (URL-Native):**
    - Contract code and state are encoded in URLs (`/sc/{payload}`).
    - Contract calls are embedded within transactions, specifying `contractId`, `action`, `entrypoint`, `input`, and pre/post state hashes.
    - Mock runtime for token operations with future plans for deterministic WASM execution, gas metering, and state Merkle commitments.
- **Rewards & Staking System:**
    - **Reward Types:** Tip Rewards (validating orphaned transactions), Stake Rewards (for validators), Witness Rewards (when transactions are referenced).
    - **Staking:** Validators stake tokens with a `minStakeAmount` and `unstakeCooldownMs`.
- **Checkpoint & Finality Proofs:**
    - Periodic checkpoints (60s intervals) signed by staked validators.
    - Finality proofs embedded as URL query parameters (`?proof={encodedProof}`).
    - Enables standalone, trustless verification from URLs by checking validator signatures and weight thresholds against a genesis-rooted chain.
    - The proof includes `checkpointId`, `checkpointHeight`, `merkleRoot`, `signatureCount`, `totalValidatorWeight`, and an array of validator signatures with public keys and weights.

**Feature Specifications:**
- **Wallet:** Client-side library for key management and transaction creation.
- **Node:** Validator and API server handling mempool, consensus, and state.
- **Faucet:** Testnet component for distributing Rinku coins.
- **Explorer:** React-based UI for ledger visualization and interaction.

**System Design Choices:**
- **Trustless Verification:** The URL-native design, combined with finality proofs, allows for complete cryptographic validation of transactions and the ledger state without relying on a centralized node.
- **Decentralization:** No single coordinator; consensus emerges from weighted votes.
- **Security:** Comprehensive security analysis and attack vector defenses are documented in `SECURITY.md`. Genesis serves as the root of trust for checkpoint verification.

### External Dependencies
- **npm workspaces:** For monorepo management.
- **React:** For the explorer frontend.
- **Vite:** For fast frontend development with React.
- **Express:** For building API servers (node, faucet).
- **Web Crypto API:** For all cryptographic operations (e.g., ed25519 signatures).
- **pako:** For DEFLATE compression in URL payloads.
- **vitest:** For comprehensive testing.

### Multi-Node Networking

**Environment Variables:**
- `NODE_PORT`: Port for node API (default: 3001)
- `NODE_ID`: Unique node identifier (auto-generated if not set)
- `NODE_PEERS`: Comma-separated list of peer URLs (e.g., "http://peer1:3001,http://peer2:3001")
- `RINKU_DATA_DIR`: Directory for persistence (default: .rinku-data)
- `SELF_URL`: This node's public URL for peer discovery
- `MAX_PEERS`: Maximum number of peers to maintain (default: 50)
- `DISCOVERY_ENABLED`: Enable/disable auto-discovery (default: true)

**Auto-Discovery (Gossip Protocol):**
Nodes automatically discover each other through gossip-based peer exchange:
1. When syncing with a peer, nodes also fetch that peer's peer list
2. New peers are added automatically (up to MAX_PEERS limit)
3. Nodes filter out themselves and duplicates
4. Only online peers are shared during discovery
5. Sync loop auto-starts when first peer is discovered

**Peer Sync Endpoints:**
- `GET /api/sync/status`: Node status (merkleRoot, dagSize, tips)
- `GET /api/sync/transactions`: All transactions with public keys
- `GET /api/sync/peers`: List of known peers with status
- `GET /api/sync/discovery`: Discovery config and peer counts
- `POST /api/sync/announce`: Announce this node to a peer
- `POST /api/sync/force`: Force sync with all peers

**Security Protections:**
The peer discovery system includes SSRF protections that block:
- Loopback addresses (127.0.0.0/8, ::1)
- Private IP ranges (10.x, 172.16-31.x, 192.168.x)
- Link-local addresses (169.254.x, fe80::)
- IPv6 private ranges (fc00::/7)
- Reserved/documentation addresses
- Localhost and .local/.internal domains

**Production Deployment Note:**
For full SSRF protection in production, combine with network-level egress restrictions to block outbound connections to internal networks.

### Performance Optimizations

**Memory Management:**
- `MAX_DAG_NODES`: Maximum in-memory transactions (default: 300). Configurable via environment variable.
- `PRUNE_INTERVAL_MS`: How often to check for pruning (default: 30000ms).
- Time-based pruning keeps the N most recent transactions by timestamp, enforcing hard cap regardless of tip count.
- Account state (balances, nonces) is always preserved regardless of pruning.
- Checkpoints provide historical verification for pruned transactions.
- **Witness tracking**: Uses TTL-based Map with O(1) head-pointer queue (1-hour window) to prevent duplicate rewards. Pruning advances a pointer instead of shifting, with periodic compaction at 10k entries.
- **Tip Explosion Fix (Jan 2026):** Removed unconditional tip protection from pruning to prevent DAG growth beyond MAX_DAG_NODES when parent URLs fail to resolve.

**Snapshot Optimizations (Jan 2026):**
- DAG nodes store compact hash-based URLs (`/tx/h/{hash}`) instead of full self-crawlable URLs.
- This prevents exponential URL growth where each URL would embed full parent URLs recursively.
- Memory reduced from ~500 MB at 70 nodes to ~10 MB at 78 nodes.

**Checkpoint-Bounded Self-Crawlable URLs (Jan 2026):**
- New `/txp/{payload}` URL format for self-crawlable proof bundles.
- Bundles contain full transaction ancestry back to the last finalized checkpoint (~60 seconds).
- `SelfCrawlableBundle` type includes `tx`, `hash`, `parents[]`, `truncatedParents[]`, and optional `checkpointAnchor`.
- API endpoint: `GET /api/tx/:hash/proof` returns the self-crawlable bundle and proof URL.
- Preserves "link is the proof" property while keeping URL sizes bounded by checkpoint interval.
- Verification function `verifySelfCrawlableBundle()` validates bundle structure.

**Per-Transaction Finality (Jan 2026):**
- Each transaction receives `FinalityMetadata` when included in a checkpoint: `checkpointId`, `checkpointHeight`, `finalizedAt`.
- `CheckpointService.onCheckpoint()` callback triggers `consensus.stampFinalityForAll()` to stamp all unfinalized transactions.
- `DAG.buildSelfCrawlableBundle()` stops recursion at finalized transactions and creates `TruncatedParentRef` entries.
- Each truncated parent carries its own `CheckpointAnchor` (checkpointId, merkleRoot, height, signatureCount).
- Multi-branch DAGs properly preserve per-branch checkpoint anchors for independent verification.

**Self-Contained Merkle Proofs (Jan 2026):**
- Transaction merkle tree infrastructure: `getTransactionMerkleRoot()` and `getTransactionMerkleProof()` functions.
- `TransactionMerkleProof` type: `{ proof: string[]; index: number; txMerkleRoot: string }`.
- Checkpoints store `txMerkleRoot` and `txHashes[]` snapshot at creation time for consistent proof generation.
- `TruncatedParentRef` includes full transaction data and merkle proof for self-contained verification.
- `buildSelfCrawlableBundle()` is async to support on-demand merkle proof embedding.
- Verification flow: recompute tx hash → verify against merkle proof → verify proof against txMerkleRoot → verify checkpoint signatures.
- Proofs remain valid regardless of subsequent DAG changes (new transactions, pruning) since they use checkpoint-snapshotted hashes.

**Batched Operations:**
- Merkle root updates are batched every 5 seconds (not per-transaction).
- Weight calculations are batched every 30 seconds.
- API responses are cached for 5 seconds with DAG-size-based invalidation.

### Dynamic Gas Fee Model (Jan 2026)

**Gas Pricing:**
- Demand-based pricing using moving average of last 100 transaction fees
- Price range: minFee (0.001) to maxFee (100), base fee 0.01
- Demand multiplier scales up to 2x based on recent transaction volume
- Fee field required in all SignedTransaction types

**Fee Distribution:**
- 50% burned (permanently removed from circulation for deflation)
- 50% distributed to active validators proportionally by stake weight
- Faucet and genesis transactions exempt from gas fees (fee: 0)

**Fee Validation Flow:**
1. POST /api/tx validates fee against GasService min/max bounds
2. Consensus.validateTransaction checks balance covers amount + fee
3. StateManager.applyTransaction deducts total (amount + fee) from sender
4. GasService.recordFee() tracks burn/validator portions
5. RewardsService.distributeFeeToValidators() credits validator balances

**API Endpoints:**
- `GET /api/gas/price`: Current gas price, min, max, avg of last 100
- `GET /api/gas/stats`: Total burned, total to validators, transaction count
- `GET /api/gas/config`: Fee multiplier, burn/validator percentages

**Design Compatibility:**
- Dynamic fees work with URL-native proofs because checkpoint inclusion proves the fee was valid at submission time
- The fee is frozen in the transaction hash and checkpoint verification ensures historical validity
- Gas service state persisted in node snapshots for restart recovery

**Scaling Notes:**
- Current config handles 300+ nodes with ~50 MB heap on Replit.
- For production, increase `MAX_DAG_NODES` on infrastructure with more memory.
- Linear memory growth: approximately 150-200 KB per transaction in memory.

### Tokenomics System (Jan 2026)

**Supply & Emission:**
- Max Supply: 30,000,000 RKU (hard cap)
- Genesis Allocation: 6,000,000 RKU (3M treasury, 2M staking rewards, 1M faucet)
- Remaining for Emission: 24,000,000 RKU distributed via checkpoint rewards
- Initial Checkpoint Reward: 150 RKU per checkpoint
- Halving Interval: Every 210,000 checkpoints
- Minimum Reward: 4.6875 RKU (after 5 halvings)

**Halving Schedule:**
| Epoch | Start Height | Reward (RKU) |
|-------|--------------|--------------|
| 0     | 0            | 150.0000     |
| 1     | 210,000      | 75.0000      |
| 2     | 420,000      | 37.5000      |
| 3     | 630,000      | 18.7500      |
| 4     | 840,000      | 9.3750       |
| 5     | 1,050,000    | 4.6875       |

**Reward Distribution (WPoS):**
- 70% distributed by stake weight (amount staked)
- 30% distributed by account age (days since first transaction)
- Rewards credited to validator balances on each checkpoint

**Slashing Penalties:**
- Double Signing: 15% of stake slashed
- Invalid Checkpoint: 25% of stake slashed
- Liveness Failure: 5% after missing 3 consecutive checkpoints
- Repeat Liveness: 10% for subsequent failures within 30 days

**Unbonding Queue:**
- 14-day unbonding period for unstaking
- Stake remains slashable during unbonding
- Processed automatically on each checkpoint

**Deflation Mechanics:**
- Gas fees: 50% burned, 50% to validators
- Net deflation when burn rate exceeds emission
- Total burned tracked in tokenomics stats

**API Endpoints:**
- `GET /api/tokenomics/supply`: Current supply stats, circulating, emitted, burned
- `GET /api/tokenomics/emission`: Halving schedule, current epoch, reward rate
- `GET /api/tokenomics/slashing`: Slashing config, events history, unbonding queue
- `GET /api/tokenomics/slashing/:validator`: Validator-specific slash history

**Explorer Integration:**
- Tokenomics tab in Explorer UI
- Supply overview with circulating and emission stats
- Emission schedule table with active epoch indicator
- Slashing rules display and event history