# Rinku: DAG & Consensus Model

> "rinku: a url-native distributed ledger, where links are the data and the proof."

## Overview

Rinku is a URL-native distributed ledger where the entire chain state is encoded in cryptographically-linked URLs. Unlike traditional blockchains that require node infrastructure for verification, Rinku transactions carry their own proof of validity.

---

## 1. DAG Structure

### Transaction Model

Each transaction is a node in a Directed Acyclic Graph (DAG):

```
Transaction {
  from: string       // Sender address (public key hash)
  to: string         // Recipient address
  amount: number     // Transfer amount
  nonce: number      // Sender's transaction count
  tipUrls: string[]  // Parent transaction URLs (DAG links)
  sig: string        // Ed25519 signature
  ts: number         // Timestamp
}
```

### DAG Node (In-Memory)

```
DAGNode {
  tx: Transaction           // The transaction data
  hash: string              // SHA-256 hash of transaction
  url: string               // Compact URL (/tx/h/{hash})
  parentHashes: string[]    // Resolved parent hashes
  weight: number            // Cumulative weight for consensus
  confirmed: boolean        // Whether transaction is confirmed
  finality?: FinalityMetadata  // Checkpoint finality info
}
```

### Parent Linking

Every transaction references 1+ parent transactions via `tipUrls`. This creates the DAG:

```
    [Genesis]
       /  \
     [A]   [B]      <- Both reference genesis
       \   /
        [C]         <- References both A and B
       /   \
     [D]   [E]      <- Fork: both reference C
       \   /
        [F]         <- Merge: references both D and E
```

**Key properties:**
- No cycles allowed (verified on insertion)
- Multiple parents enable parallel transaction processing
- Forks are resolved by cumulative weight

---

## 2. URL Formats

### Compact Hash URL (Storage)
Used internally for memory efficiency:
```
/tx/h/{hash}
Example: /tx/h/abc123def456...
```

### Self-Crawlable Proof URL (External)
Used for trustless verification without API access:
```
/txp/{base64url-deflated-bundle}
```

The bundle contains everything needed to verify:
```
SelfCrawlableBundle {
  tx: Transaction           // Full transaction data
  hash: string              // Transaction hash
  parents: SelfCrawlableBundle[]  // Recursive parent bundles
  truncatedParents: TruncatedParentRef[]  // Finalized parents (with proofs)
  checkpointAnchor?: CheckpointAnchor     // Finality proof
}
```

---

## 3. Checkpoint System

### Purpose
Checkpoints provide periodic finality snapshots signed by validators. They enable:
- Bounded proof URLs (recursion stops at finalized transactions)
- Historical verification after DAG pruning
- Trust anchor chain back to genesis

### Checkpoint Structure
```
Checkpoint {
  checkpointId: string          // Unique identifier
  height: number                // Sequential height (0 = genesis)
  merkleRoot: string            // DAG state merkle root
  txMerkleRoot: string          // Transaction merkle root
  txHashes: string[]            // Snapshot of all tx hashes at creation
  tipCount: number              // Number of DAG tips
  totalTransactions: number     // Total transactions in DAG
  totalWeight: number           // Network weight
  validatorSetHash: string      // Hash of validator set
  previousCheckpointId: string  // Chain to previous checkpoint
  validators: ValidatorEntry[]  // Current validator set
  timestamp: number             // Creation time
  signatures: ValidatorSignature[]  // Multi-sig from validators
}
```

### Finality Flow
1. Checkpoint created every 60 seconds
2. Validators sign checkpoint with their stake weight
3. When threshold reached, checkpoint is finalized
4. All unfinalized transactions receive `FinalityMetadata`:
   ```
   FinalityMetadata {
     checkpointId: string
     checkpointHeight: number
     finalizedAt: number
   }
   ```

---

## 4. Self-Contained Proofs

### The Problem
Traditional blockchains require API calls to verify transactions. This creates infrastructure dependency.

### Rinku's Solution
Each URL contains its complete verification chain:

```
┌─────────────────────────────────────────────────────────┐
│  Self-Crawlable Bundle                                  │
│  ┌─────────────────────────────────────────────────┐    │
│  │  Transaction + Hash + Signature                 │    │
│  └─────────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────────┐    │
│  │  Parent Bundles (recursive, unfinalized)        │    │
│  │  - Each parent is itself a complete bundle      │    │
│  └─────────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────────┐    │
│  │  Truncated Parents (finalized, with proofs)     │    │
│  │  - Transaction data                             │    │
│  │  - Merkle proof against checkpoint              │    │
│  │  - Checkpoint anchor with validator signatures  │    │
│  └─────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
```

### Truncated Parent Reference
When a parent is finalized, instead of embedding its full ancestry, we embed:

```
TruncatedParentRef {
  hash: string                      // Transaction hash
  tx: Transaction                   // Full transaction data
  merkleProof: TransactionMerkleProof  // Proof of inclusion
  checkpointAnchor: CheckpointAnchor   // Finality proof
}

TransactionMerkleProof {
  proof: string[]      // Sibling hashes for merkle path
  index: number        // Position in merkle tree
  txMerkleRoot: string // Root to verify against
}

CheckpointAnchor {
  checkpointId: string
  merkleRoot: string
  txMerkleRoot: string
  height: number
  signatureCount: number
}
```

### Verification Flow (No API Required)

```
1. Receive self-crawlable URL
   │
2. Decode bundle from URL
   │
3. For each parent:
   │
   ├─► If full bundle: recurse (step 2)
   │
   └─► If truncated parent:
       │
       a. Recompute transaction hash from tx data
       │
       b. Verify hash matches merkle proof leaf
       │
       c. Compute merkle root from proof
       │
       d. Verify computed root == checkpoint.txMerkleRoot
       │
       e. Verify checkpoint anchor signatures
       │
       f. Trace checkpoint chain to trusted genesis
   │
4. Verify current transaction signature
   │
5. Transaction is cryptographically proven ✓
```

---

## 5. Weight-Based Consensus

### Sybil Resistance Formula
```
weight = (account_age_days * 0.3) + (balance * 0.7)
```

### Conflict Resolution
When forks occur (multiple transactions reference same parent):
1. Calculate cumulative weight of each branch
2. Branch with higher weight wins
3. Weight includes all descendants

### Account Weight Calculation
```
AccountWeight {
  address: string
  balance: number
  firstSeen: number  // Timestamp
  age_days: (now - firstSeen) / 86400000
  weight: age_days * 0.3 + balance * 0.7
}
```

---

## 6. Memory Management

### DAG Pruning
- `MAX_DAG_NODES`: Maximum in-memory transactions (default: 300)
- Time-based pruning keeps N most recent by timestamp
- Account state (balances, nonces) preserved regardless of pruning

### Why Pruning Works
- Finalized transactions have merkle proofs in checkpoints
- Checkpoint chain provides historical verification
- Self-crawlable URLs embed all needed ancestry

### Memory Footprint
- ~150-200 KB per transaction in memory
- Compact hash URLs prevent exponential growth
- Checkpoint-bounded proofs keep URL sizes manageable

---

## 7. Trust Model

### Genesis Bootstrap
```
Genesis Checkpoint
       │
       ▼
  Checkpoint 1 (signed by validators)
       │
       ▼
  Checkpoint 2 (signed by validators)
       │
       ▼
     ...
       │
       ▼
  Latest Checkpoint
       │
       ▼
  Unfinalized Transactions
```

### What You Trust
1. **Genesis checkpoint** - Root of trust (hardcoded/distributed)
2. **Validator set** - Defined in each checkpoint
3. **Cryptographic primitives** - Ed25519, SHA-256, DEFLATE

### What You DON'T Need to Trust
- Any specific node/API server
- Network connectivity after receiving URL
- Third-party verification services

---

## Summary

| Component | Purpose |
|-----------|---------|
| DAG | Parallel transaction graph with weighted consensus |
| Compact URLs | Memory-efficient internal storage |
| Self-Crawlable URLs | Infrastructure-independent proofs |
| Checkpoints | Periodic finality + bounded proof size |
| Merkle Proofs | Cryptographic inclusion verification |
| Weight Formula | Sybil resistance via age + stake |
| Genesis | Root of trust for checkpoint chain |

**The key innovation**: Every URL is a complete cryptographic proof. No API calls needed. The link IS the proof.
