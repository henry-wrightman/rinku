# Rinku: A URL-Native Distributed Ledger

**Abstract.** A distributed ledger where the entire state exists as cryptographically-linked URLs. Transactions are self-contained proofs embedded in URLs, enabling trustless verification without infrastructure dependency. We propose a DAG-based consensus mechanism with weight-based Sybil resistance combining stake and account age. The system achieves per-transaction finality through periodic checkpoints, implements deflationary tokenomics with halving-based emission, and supports extensible smart contracts. The result is a ledger where the link itself is the proof.

## 1. Introduction

Traditional blockchains require nodes to verify state. Users must trust infrastructure providers or run their own nodes. We eliminate this dependency by encoding the complete verification path in URLs.

The problem with existing systems:
1. **Infrastructure dependency** - Verification requires trusted nodes
2. **State opacity** - Users cannot independently verify without syncing
3. **Proof complexity** - Light client proofs require specialized tooling

Rinku solves this by making URLs self-verifying. A transaction URL contains its ancestry back to a finalized checkpoint, cryptographic proofs, and sufficient data for complete validation.

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
  sig: string,       // ECDSA signature
  hash: string       // SHA-256 hash of tx fields
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
- Checkpoint signatures

### 2.3 Self-Contained Verification

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
- `ageWeight` = Account age in days / Oldest account age

This creates Sybil resistance: new accounts with no stake have minimal weight. Established, staked accounts anchor consensus.

## 4. Checkpoints and Finality

### 4.1 Checkpoint Creation

Every 15 seconds (configurable), the network produces a checkpoint:

```
{
  id: string,           // SHA-256 hash
  height: number,       // Sequential checkpoint number
  timestamp: number,    // Creation time
  merkleRoot: string,   // Root of all transaction hashes
  previousId: string,   // Link to prior checkpoint
  txHashes: string[],   // Transactions in this checkpoint
  signatures: string[]  // Validator signatures
}
```

### 4.2 Finality

A transaction achieves finality when included in a checkpoint. The checkpoint's Merkle root commits to all included transactions. Once finalized:
- The transaction cannot be reversed
- Proof URLs can be bounded to checkpoint ancestry
- State is frozen at that point

### 4.3 Finality Metrics

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

Account state is committed to a Merkle tree. Any account balance can be proven with O(log n) proof size. Proofs are included in transaction URLs for self-contained verification.

## 6. Tokenomics

### 6.1 Supply

- **Maximum Supply:** 30,000,000 RKU (hard cap)
- **Genesis Allocation:** 6,000,000 RKU
  - 3,000,000 RKU - Treasury
  - 2,000,000 RKU - Staking rewards reserve
  - 1,000,000 RKU - Faucet distribution
- **Emission:** 24,000,000 RKU via checkpoint rewards

### 6.2 Emission Schedule

Rewards halve every 210,000 checkpoints:

| Epoch | Checkpoints | Reward/Checkpoint |
|-------|-------------|-------------------|
| 0     | 0-209,999   | 150 RKU           |
| 1     | 210,000-419,999 | 75 RKU        |
| 2     | 420,000-629,999 | 37.5 RKU      |
| 3     | 630,000-839,999 | 18.75 RKU     |
| 4     | 840,000-1,049,999 | 9.375 RKU   |
| 5+    | 1,050,000+  | 4.6875 RKU        |

### 6.3 Reward Distribution

Checkpoint rewards distributed to active validators using Weighted Proof-of-Stake:
- 70% proportional to stake amount
- 30% proportional to account age

This rewards both capital commitment and long-term participation.

### 6.4 Deflationary Pressure

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
- **Storage:** ~150-200 KB per transaction

### 11.2 Scalability Path

- DAG structure enables horizontal scaling
- Checkpoint parallelization
- State sharding (future work)
- Layer 2 solutions for high-frequency use cases

## 12. Conclusion

Rinku demonstrates that distributed ledger state can exist entirely in URLs. By encoding transactions, proofs, and ancestry in self-contained URLs, we eliminate infrastructure dependency for verification.

The combination of DAG-based consensus, weighted proof-of-stake, checkpoint finality, and deflationary tokenomics creates a functional distributed ledger. The URL-native design enables novel use cases: embed payment proofs in QR codes, verify transactions offline, share ledger state via hyperlinks.

The link is the ledger.

---

## References

1. Nakamoto, S. (2008). Bitcoin: A Peer-to-Peer Electronic Cash System.
2. Popov, S. (2018). The Tangle. IOTA Foundation.
3. Buterin, V. (2014). Ethereum: A Next-Generation Smart Contract Platform.

## Appendix A: Cryptographic Primitives

- **Signatures:** ECDSA P-256 with SHA-256
- **Hashing:** SHA-256 for transactions, Merkle trees, checkpoints
- **Key Derivation:** 40-character fingerprint from public key hash

## Appendix B: URL Format Specification

Transaction URL:
```
/tx/{base64url(deflate(json(transaction)))}
```

Proof Bundle URL:
```
/txp/{base64url(deflate(json({
  tx: transaction,
  ancestry: [parent_txs...],
  checkpoint: checkpoint_data,
  merkleProof: [proof_hashes...]
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
  "checkpointInterval": 15000,
  "unbondingPeriod": 1209600000
}
```
