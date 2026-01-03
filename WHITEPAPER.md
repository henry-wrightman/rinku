# Rinku: A URL-Native Distributed Ledger

**Abstract.** A distributed ledger where the entire state exists as cryptographically-linked URLs. Transactions are self-contained proofs embedded in URLs, enabling trustless verification without infrastructure dependency. We propose a DAG-based consensus mechanism with weight-based Sybil resistance combining stake and account age. The system achieves per-transaction finality through periodic checkpoints, implements deflationary tokenomics with halving-based emission, and supports extensible smart contracts. The result is a ledger where the link itself is the proof.

## 1. Introduction

Traditional blockchains require nodes to verify state. Users must trust infrastructure providers or run their own nodes. We eliminate this dependency by encoding the complete verification path in URLs.

The problem with existing systems:
1. **Infrastructure dependency** - Verification requires trusted nodes
2. **State opacity** - Users cannot independently verify without syncing
3. **Proof complexity** - Light client proofs require specialized tooling

Rinku solves this by making URLs self-verifying. A transaction URL contains its ancestry back to a finalized checkpoint, cryptographic proofs, and sufficient data for complete validation. Through aggressive compression, complete finality proofs fit within standard URL length limits—including QR codes.

## 2. URL-Native Transactions

### 2.1 Transaction Structure

A transaction contains:

```
{
  from: string,      // Sender fingerprint (40-char hex)
  to: string,        // Recipient fingerprint
  amount: number,    // Transfer amount
  fee: number,       // Gas fee
  nonce: number,     // Sender's sequence number
  tipUrls: string[], // References to DAG tips (0-2 parents)
  ts: number,        // Unix timestamp
  sig: string        // ECDSA signature
}
```

### 2.2 URL Encoding

Transactions are encoded as:
1. JSON serialization
2. DEFLATE compression (pako)
3. Base64url encoding
4. Embedded in URL path: `/tx/{payload}`

A complete proof bundle uses: `/txp/{payload}` containing:
- The transaction
- Ancestry chain to last checkpoint
- Merkle proofs for verification
- Checkpoint anchor data

### 2.3 Proof Size Analysis

Empirical measurement of encoded proof bundle sizes:

| Proof Type | Transactions | JSON Size | URL Length |
|------------|--------------|-----------|------------|
| Single tx | 1 | 558 bytes | 389 chars |
| 1-depth ancestry | 2 | 919 bytes | 417 chars |
| 2-depth ancestry | 3 | 1,280 bytes | 432 chars |
| 5-depth ancestry | 6 | 2,363 bytes | 469 chars |
| 10-depth ancestry | 11 | 4,169 bytes | 536 chars |
| DAG 3-depth (15 txs) | 15 | 7,003 bytes | 640 chars |
| DAG 4-depth (31 txs) | 31 | 14,379 bytes | 887 chars |

DEFLATE compression achieves ~85-95% reduction on transaction JSON.

### 2.4 Platform Compatibility

All proof bundles fit within standard limits:

| Platform | Limit | 5-depth proof | 31-tx DAG proof |
|----------|-------|---------------|-----------------|
| QR Code (alphanumeric) | 4,296 chars | ✓ | ✓ |
| Internet Explorer 11 | 2,083 chars | ✓ | ✓ |
| Firefox | 65,536 chars | ✓ | ✓ |
| Chrome/Edge | 2 MB | ✓ | ✓ |
| nginx/Apache default | 8,192 chars | ✓ | ✓ |

This enables true offline verification: a payment proof can be embedded in a QR code, shared via messaging, or stored as a bookmark—no network required for validation.

### 2.5 Self-Contained Verification

Any party receiving a transaction URL can:
1. Decode and decompress the payload
2. Verify the signature against the sender's public key
3. Validate the hash integrity
4. Trace ancestry to a finalized checkpoint
5. Verify Merkle inclusion proofs

No external queries required. The URL is the proof.

## 3. DAG-Based Consensus

### 3.1 Structure

Unlike linear blockchains, Rinku uses a Directed Acyclic Graph. Each transaction references 0-2 prior transactions (tips), creating a mesh of dependencies.

```
    [tx1] ← [tx3] ← [tx5]
              ↖      ↗
    [tx2] ←  [tx4]
```

Benefits:
- **Parallelism** - Multiple transactions can be added concurrently
- **Reduced contention** - No single chain bottleneck
- **Natural ordering** - Causal relationships preserved

### 3.2 Conflict Resolution

When conflicting transactions exist (e.g., double-spend attempts), the transaction with greater cumulative weight wins. Weight flows from tips backward through the DAG.

### 3.3 Weight Calculation

Transaction weight derives from the originating account's weighted proof-of-stake:

```
weight = (0.7 × stakeWeight) + (0.3 × ageWeight)
```

Where:
- `stakeWeight` = Account's staked balance / Total staked
- `ageWeight` = min(accountAgeDays, 365) / 365

The age component is capped at 1 year to prevent early-adopter lock-in and reduce incentive for account farming.

This creates Sybil resistance: new accounts with no stake have minimal weight. Established, staked accounts anchor consensus.

### 3.4 Age Weight Mitigations

To prevent gaming of the age component:
- **Capped duration**: Age weight saturates at 365 days
- **Log-scale consideration**: Future versions may apply logarithmic scaling
- **Staked duration**: Alternative metric measuring continuous stake time rather than account creation

## 4. Checkpoints and Finality

### 4.1 Checkpoint Creation

Every 15 seconds (configurable), the network produces a checkpoint:

```
{
  id: string,           // SHA-256 hash
  height: number,       // Sequential checkpoint number
  timestamp: number,    // Creation time
  txMerkleRoot: string, // Merkle root of transaction hashes
  stateRoot: string,    // Merkle root of account states
  previousId: string,   // Link to prior checkpoint
  txHashes: string[],   // Transactions in this checkpoint
  signatures: string[], // Validator signatures
  signatureCount: number
}
```

### 4.2 Consensus Protocol

Checkpoint finality requires Byzantine fault tolerance:

1. **Leader Selection**: Weighted round-robin proportional to stake. At height `h`, the leader is selected by:
   - Compute cumulative stake thresholds for each validator
   - Select validator whose threshold range contains `(h × primeMultiplier) mod totalStake`
   - This ensures validators lead checkpoints proportional to their stake weight

2. **Quorum Threshold**: A checkpoint is valid when signed by validators representing ≥ 2/3 of total staked weight.

3. **Fork Choice Rule**: Before finality, nodes follow the heaviest-weight chain. After checkpoint finalization, that branch becomes canonical.

4. **Conflicting Checkpoints**: If a validator signs conflicting checkpoints at the same height, they are slashed for double-signing (15% of stake).

5. **Validator Set Updates**: The active validator set is determined by stake positions at the previous checkpoint. Changes take effect at the next checkpoint boundary.

### 4.3 Finality

A transaction achieves finality when included in a checkpoint with sufficient validator signatures. The checkpoint's Merkle roots commit to all included transactions and resulting state. Once finalized:
- The transaction cannot be reversed
- Proof URLs are bounded to checkpoint ancestry
- State is frozen at that point

### 4.4 Finality Metrics

The network tracks:
- Average time-to-finality
- Median and P95 finality times
- Pending transaction count
- Checkpoint latency
- Throughput (transactions per second)

Current performance: ~15-30s average finality, 100% finality rate at steady state.

## 5. State Management

### 5.1 Account Model

Each account maintains:

```
{
  fingerprint: string,    // 40-char public key hash
  balance: number,        // Current balance
  nonce: number,          // Transaction counter
  firstTxTimestamp: number // Account creation time
}
```

### 5.2 State Transitions

Transactions modify state atomically:
1. Verify sender has sufficient balance (amount + fee)
2. Decrement sender balance by (amount + fee)
3. Increment recipient balance by amount
4. Increment sender nonce
5. Process fee (50% burn, 50% to validators)

### 5.3 Merkle State Proofs

Account state is committed to a Merkle tree with root included in each checkpoint (`stateRoot`). Any account balance can be proven with O(log n) proof size. Proofs are anchored to checkpoint state roots for self-contained verification.

## 6. Tokenomics

### 6.1 Supply

- **Maximum Supply:** 30,000,000 RKU (hard cap, enforced)
- **Genesis Allocation:** 6,000,000 RKU
  - 3,000,000 RKU - Treasury
  - 2,000,000 RKU - Staking rewards reserve
  - 1,000,000 RKU - Faucet distribution (testnet only)
- **Emission:** Up to 24,000,000 RKU via checkpoint rewards

### 6.2 Emission Schedule

Rewards halve every 210,000 checkpoints (~36.5 days at 15s intervals):

| Epoch | Checkpoints | Reward/Checkpoint | Cumulative Emission |
|-------|-------------|-------------------|---------------------|
| 0 | 0-209,999 | 150 RKU | 31,500,000 RKU* |
| 1 | 210,000-419,999 | 75 RKU | +15,750,000 RKU |
| 2 | 420,000-629,999 | 37.5 RKU | +7,875,000 RKU |
| 3 | 630,000-839,999 | 18.75 RKU | +3,937,500 RKU |
| 4 | 840,000-1,049,999 | 9.375 RKU | +1,968,750 RKU |
| 5+ | 1,050,000+ | 4.6875 RKU | until cap |

*Theoretical maximum; actual emission stops when `totalSupply >= maxSupply`.

**Hard Cap Enforcement**: Once total circulating supply reaches 30,000,000 RKU, checkpoint rewards drop to 0. The floor reward of 4.6875 RKU only applies while supply remains below the cap.

### 6.3 Halving Rationale

The 36.5-day halving interval (vs. Bitcoin's 4 years) enables:
- Rapid initial distribution for network bootstrapping
- Earlier transition to fee-based validator economics
- Predictable supply schedule completion within ~1 year

### 6.4 Reward Distribution

Checkpoint rewards distributed to active validators using Weighted Proof-of-Stake:
- 70% proportional to stake amount
- 30% proportional to capped account age

This rewards both capital commitment and long-term participation.

### 6.5 Deflationary Pressure

Gas fees create deflation:
- 50% of each fee is burned (permanently removed)
- 50% distributed to validators

Net supply decreases when burn rate exceeds emission rate.

## 7. Dynamic Gas Fees

### 7.1 Pricing Model

Gas price adjusts based on network demand:

```
currentPrice = baseFee × demandMultiplier
demandMultiplier = 1 + (recentTxCount / targetTxCount)
```

Bounded by:
- Minimum: 0.001 RKU
- Maximum: 100 RKU

### 7.2 Fee Validation

Transactions must include a fee meeting current minimum. Insufficient fees result in rejection. This prevents spam while allowing market-based pricing.

## 8. Staking and Slashing

### 8.1 Staking

Any account can stake RKU to become a validator:
1. Lock tokens in staking contract
2. Gain weight in consensus
3. Earn proportional checkpoint rewards
4. Subject to slashing for misbehavior

Minimum stake: 100 RKU

### 8.2 Slashing Penalties

| Violation | Penalty |
|-----------|---------|
| Double signing | 15% of stake |
| Invalid checkpoint | 25% of stake |
| Liveness failure (3+ missed) | 5% of stake |
| Repeat liveness (within 30 days) | 10% of stake |

### 8.3 Unbonding

Unstaking requires a 14-day unbonding period:
- Stake remains slashable during unbonding
- Prevents quick exit after misbehavior
- Processed automatically each checkpoint

## 9. Smart Contracts (Work in Progress)

### 9.1 Architecture

Contracts are URL-encoded programs with:
- Immutable code (WASM bytecode)
- Mutable state (key-value storage)
- Defined interface (callable methods)

### 9.2 Execution Model

Contract calls are embedded in transactions:
```
{
  ...transaction fields,
  contractCall: {
    contractId: string,
    method: string,
    args: any[]
  }
}
```

### 9.3 Current Status

The contract framework is implemented with:
- Deploy, call, and query interfaces
- State persistence
- Gas metering hooks

Full WASM execution is under development. Current implementation uses a simulated runtime for interface validation.

## 10. Network Protocol

### 10.1 Peer Discovery

Nodes discover peers via gossip protocol:
1. Exchange known peer lists
2. Validate peer liveness
3. Maintain connection pool (max 50 peers)

### 10.2 State Synchronization

New nodes sync via:
1. Request latest checkpoint from peers
2. Download transactions since checkpoint
3. Replay to reconstruct state
4. Verify Merkle roots match

### 10.3 Transaction Propagation

Submitted transactions:
1. Validate locally
2. Add to mempool
3. Broadcast to connected peers
4. Include in next checkpoint

## 11. Performance

### 11.1 Current Metrics

- **Throughput:** 3-5 TPS (single node testnet)
- **Finality:** 15-30 seconds average
- **Memory:** ~50 MB heap for 300 transactions
- **Proof Size:** 400-900 chars for typical proofs (fits in QR codes)

### 11.2 Scalability Path

- DAG structure enables horizontal scaling
- Checkpoint parallelization
- State sharding (future work)
- Layer 2 solutions for high-frequency use cases

## 12. Conclusion

Rinku demonstrates that distributed ledger state can exist entirely in URLs. By encoding transactions, proofs, and ancestry in self-contained URLs, we eliminate infrastructure dependency for verification.

Through aggressive DEFLATE compression, complete finality proofs—including multi-transaction ancestry chains—fit within 500-900 characters. This enables genuine offline verification: embed payment proofs in QR codes, verify transactions without network access, share ledger state via hyperlinks.

The combination of DAG-based consensus, weighted proof-of-stake, checkpoint finality, and deflationary tokenomics creates a functional distributed ledger with a novel property: **the link is the proof**.

---

## References

1. Nakamoto, S. (2008). Bitcoin: A Peer-to-Peer Electronic Cash System.
2. Popov, S. (2018). The Tangle. IOTA Foundation.
3. Buterin, V. (2014). Ethereum: A Next-Generation Smart Contract Platform.
4. Castro, M. & Liskov, B. (1999). Practical Byzantine Fault Tolerance.

## Appendix A: Cryptographic Primitives

- **Signatures:** ECDSA P-256 with SHA-256 (chosen for native Web Crypto API support)
- **Hashing:** SHA-256 for transactions, Merkle trees, checkpoints
- **Key Derivation:** 40-character fingerprint from SHA-1 of public key
- **Compression:** DEFLATE (pako) for URL payload encoding

## Appendix B: URL Format Specification

Transaction URL:
```
/tx/{base64url(deflate(json(transaction)))}
```

Proof Bundle URL:
```
/txp/{base64url(deflate(json({
  tx: Transaction,
  hash: string,
  parents: SelfCrawlableBundle[],
  checkpointAnchor: {
    checkpointId: string,
    merkleRoot: string,
    height: number,
    signatureCount: number
  }
})))}
```

## Appendix C: Genesis Configuration

```json
{
  "maxSupply": 30000000,
  "genesisAllocation": {
    "treasury": 3000000,
    "stakingReserve": 2000000,
    "faucet": 1000000
  },
  "initialReward": 150,
  "halvingInterval": 210000,
  "minReward": 4.6875,
  "emissionStopsAtCap": true,
  "checkpointInterval": 15000,
  "unbondingPeriod": 1209600000,
  "quorumThreshold": 0.67,
  "ageWeightCap": 365
}
```

## Appendix D: Proof Size Benchmarks

Measured on reference implementation (vitest, Node.js):

```
Single transaction:     389 chars  (0.38 KB)
2-ancestor proof:       432 chars  (0.42 KB)
5-ancestor proof:       469 chars  (0.46 KB)
10-ancestor proof:      536 chars  (0.52 KB)
15-tx DAG proof:        640 chars  (0.63 KB)
31-tx DAG proof:        887 chars  (0.87 KB)
```

All proofs fit within QR code capacity (4,296 alphanumeric characters).
