# Partition-Tolerant Mode: Architecture Specification

**Status:** Draft v1.1
**Date:** February 2026
**Influences:** SwarmDAG (Ledger Journal, 2019), Vegvisir, CAP theorem literature
**Approach:** Option 3 (Optimistic Execution + Cascading Rollback) with Option 1 borrowing (Economic Deterrence)

---

## 1. Design Philosophy

Traditional blockchains (BTC, ETH, Solana) choose **CP** in the CAP theorem: they prioritize Consistency and Partition tolerance but sacrifice Availability — the network halts when partitions prevent quorum. This is unacceptable for mesh-native networks where partitions are expected, not exceptional.

Rinku takes a different position: **tunable consistency**. When the network is fully connected, Mysticeti-FPC delivers sub-second strong finality. When partitions occur, the network degrades gracefully to eventual consistency — transactions continue locally, and state reconciles deterministically when partitions heal.

The core invariant: **no honest user is prevented from transacting during a partition.** The cost of this guarantee is that some transactions may be rolled back during merge if they conflict with transactions from other partitions. Intentional abuse (cross-partition double-spending) is economically penalized.

### 1.1 Determinism Requirements

Every aspect of the merge protocol must produce identical results on every node given the same inputs. This document specifies the following determinism guarantees:

**Integer Accounting:** All balance comparisons and computations during merge use `u64` micro-units (1 RKU = 100,000,000 micro-RKU). Rinku already has `to_micro_units()` for proofs and hashing. The merge algorithm operates exclusively on micro-unit balances derived from the fork-point checkpoint's state root. The existing f64 runtime balances are converted to micro-units at merge entry and converted back only after reconciliation is complete. This eliminates floating-point nondeterminism entirely within the merge path.

**Canonical Ordering:** The cascade rollback replay requires a total ordering of transactions. This ordering is:
1. Primary: nonce ascending (per account)
2. Secondary: DAG depth from fork-point (topological distance)
3. Tertiary: transaction hash lexicographic ascending (deterministic tiebreaker)

This ordering is a strict total order — no two transactions can be "equal" because transaction hashes are unique. Every node computing this order on the same transaction set will produce the identical sequence.

**Contract Side Effects:** During cascade rollback replay, contract calls are **not re-executed**. Instead, the merge algorithm operates only on the balance transfers recorded in the original transaction execution (amount + gas fee). Contract storage conflicts are resolved separately via "last-write-wins by weight" on individual storage keys (see Section 7.3). This avoids nondeterminism from contract execution order.

**State Root Recomputation:** After cascade rollback produces `final_balances`, the state root is recomputed by:
1. Writing all surviving account balances (converted back to f64 from micro-units) to the account state
2. Rebuilding the Sparse Merkle Trie from the updated account set (deterministic — same accounts produce same trie)
3. The merge checkpoint's `state_root` is this recomputed root
4. Receipt root is recomputed from surviving transactions' receipts only

---

## 2. Partition Detection

### 2.1 Detection Heuristic

Rinku already tracks peer health via `PeerInfo` (fields: `is_healthy`, `consecutive_failures`, `backoff_until`) and validator sets via `state.validators: HashMap<String, Validator>`. Partition detection extends this with a new `PartitionDetector` that continuously evaluates network health.

**Inputs:**
- Known validator set: `state.validators` (addresses + stakes)
- Reachable validators: subset of `gossip.peers` where `is_healthy == true` and the peer is a known validator
- Checkpoint progress: whether new checkpoints are being created at the expected interval

**State Machine:**

```
                  ┌──────────────────────────────────────────────────┐
                  │                                                  │
                  ▼                                                  │
            ┌──────────┐    visible stake < 2/3     ┌────────────┐  │
            │  NORMAL  │ ────────────────────────►  │ SUSPECTED  │  │
            └──────────┘                            └────────────┘  │
                  ▲                                       │         │
                  │  visible stake ≥ 2/3                  │ timeout │
                  │  for T_recovery seconds               │ T_conf  │
                  │                                       ▼         │
                  │                                 ┌────────────┐  │
                  └──────────────────────────────── │ PARTITIONED│──┘
                     merge protocol completes       └────────────┘
```

**Parameters:**
| Parameter | Default | Description |
|-----------|---------|-------------|
| `T_conf` (confirmation timeout) | 30s | How long to wait in SUSPECTED before declaring partition |
| `T_recovery` (recovery window) | 10s | How long quorum must be restored before returning to NORMAL |
| `stake_visibility_threshold` | 0.6666 | Fraction of total stake that must be visible for NORMAL mode |

**Checkpoint Stall Detection (secondary signal):**
If no new checkpoint has been created for `3 × checkpoint_interval` while the node has unfinalized transactions, this is an additional signal of partition. This catches the case where individual peer connections seem healthy but the validator quorum is fragmented across partitions.

### 2.2 Integration Points

The `PartitionDetector` runs as a background task alongside the existing `GossipService` and `CheckpointService`. It reads:
- `gossip.inner.peers` for peer health status
- `state.validators` for the full validator set and stakes
- `state.checkpoints.last()` for checkpoint staleness

It writes:
- `state.partition_state: PartitionState` (new field on `InnerState`)

### 2.3 Partition Epoch

When entering PARTITIONED mode, a new `partition_epoch` is assigned (monotonically increasing u64). This epoch tags all transactions and provisional checkpoints created during the partition, enabling the merge protocol to distinguish pre-partition from during-partition state.

```rust
pub struct PartitionState {
    pub status: PartitionStatus,
    pub current_epoch: Option<u64>,
    pub epoch_start_checkpoint: Option<u64>,
    pub epoch_start_timestamp: Option<u64>,
    pub visible_validators: Vec<String>,
    pub visible_stake_pct: f64,
    pub suspected_since: Option<u64>,
}

pub enum PartitionStatus {
    Normal,
    Suspected,
    Partitioned,
}
```

---

## 3. Provisional Checkpoints

### 3.1 Concept

During a partition, the local validator subset may not meet the full 2/3 quorum threshold. Rather than halting checkpoint creation entirely, validators create **provisional checkpoints** that:
- Finalize transactions locally (enabling continued operation)
- Carry metadata indicating they are provisional
- Are subject to rollback during partition merge

### 3.2 Checkpoint Extension

The existing `Checkpoint` struct (in `rinku-core/src/types.rs`) gains new fields:

```rust
pub struct Checkpoint {
    // ... existing fields ...
    pub height: u64,
    pub hash: String,
    pub previous_hash: Option<String>,
    pub tx_merkle_root: String,
    pub state_root: String,
    pub receipt_root: String,
    pub tip_count: u32,
    pub timestamp: u64,
    pub validator_signatures: Vec<ValidatorSignature>,
    pub aggregated_signature: Option<String>,
    pub signer_bitmap: Option<Vec<u8>>,
    pub finalized_tx_hashes: Vec<String>,

    // NEW: Partition-tolerance fields
    #[serde(default)]
    pub provisional: bool,
    #[serde(default)]
    pub partition_epoch: Option<u64>,
    #[serde(default)]
    pub visible_stake_pct: Option<f64>,
}
```

### 3.3 Provisional Checkpoint Rules

1. **Creation:** The `CheckpointService` creates provisional checkpoints when:
   - `PartitionState.status == Partitioned`
   - Local visible validators have >= 1/3 stake (below this, even provisional checkpoints are too risky)
   - At least 1 local validator is available to sign

2. **Quorum Relaxation:** During partition, the quorum threshold for checkpoint acceptance drops from `0.6666` (2/3 of total stake) to `0.6666 of visible stake`. The checkpoint's `visible_stake_pct` records what fraction of total stake was visible at creation.

3. **Finality Downgrade:** Transactions finalized by provisional checkpoints are marked with `provisional_finality: true` in the DAG node metadata. They are treated as finalized for local operations but remain rollback-eligible during merge.

4. **Chain Integrity:** Provisional checkpoints still maintain `previous_hash` linkage within their partition. This creates a valid sub-chain that can be compared and merged with other partitions' sub-chains.

### 3.4 Checkpoint Upgrade

When a partition heals and the merge protocol completes:
- Provisional checkpoints that have no conflicts are **upgraded** to confirmed (set `provisional = false`)
- Provisional checkpoints containing rolled-back transactions are **superseded** by a new merge checkpoint
- The merge checkpoint's `previous_hash` links to the last pre-partition confirmed checkpoint (the fork point)

---

## 4. Local DAG Operation During Partition

### 4.1 Unchanged Behavior

The following operate identically during a partition:
- Transaction submission and validation (nonce, balance, signature checks)
- DAG tip selection and parent referencing
- Weight calculation (`age_weight * balance_weight + stake_weight`)
- Gas fee computation (EIP-1559 style dynamic pricing)
- Smart contract execution
- Fork remediation (double-spend detection within the partition)

### 4.2 Modified Behavior

- **Transaction tagging:** New transactions receive `partition_epoch` metadata matching the current epoch. This is metadata only — it does not affect the transaction hash or signature.
- **Finality semantics:** Users and dApps are informed that finality is provisional during partitions. The explorer shows a visual indicator.
- **Gossip scope:** Gossip naturally scopes to reachable peers. No code change needed — gossip simply won't reach peers in other partitions.

### 4.3 DagNode Extension

```rust
pub struct DagNode {
    // ... existing fields ...
    pub hash: String,
    pub tx: SignedTransaction,
    pub parents: Vec<String>,
    pub children: Vec<String>,
    pub weight: f64,
    pub finalized: bool,
    pub checkpoint_height: Option<u64>,
    pub received_at_ms: Option<u64>,

    // NEW: Partition-tolerance fields
    #[serde(default)]
    pub partition_epoch: Option<u64>,
    #[serde(default)]
    pub provisional_finality: bool,
}
```

---

## 5. Merge Protocol

This is the core innovation. When partitions reconnect, nodes execute a structured merge protocol inspired by SwarmDAG's EVS-based approach but extended for financial state reconciliation.

### 5.1 Merge Trigger

Merge is triggered when:
1. A node in PARTITIONED mode receives gossip from a peer whose last common checkpoint differs (indicating they were in a different partition)
2. A node discovers peers with provisional checkpoint chains that diverge from its own

**Detection mechanism:** During the gossip handshake (`TipAnnouncement`), nodes exchange:
- Their current `partition_epoch`
- Their last confirmed (non-provisional) checkpoint hash and height
- Their provisional checkpoint chain tip (if any)

If two nodes share the same last confirmed checkpoint but have different provisional chains, a merge is needed.

### 5.2 Merge Phases

```
 Phase 1          Phase 2          Phase 3          Phase 4          Phase 5
┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐
│   DAG    │───►│ Conflict │───►│  Weight  │───►│ Cascade  │───►│  State   │
│ Exchange │    │Detection │    │Resolution│    │ Rollback │    │ Reconcile│
└──────────┘    └──────────┘    └──────────┘    └──────────┘    └──────────┘
```

#### Phase 1: DAG Exchange

Both partitions exchange their DAG state since the fork point (last common confirmed checkpoint):

```rust
pub struct PartitionDAGDelta {
    pub partition_epoch: u64,
    pub fork_point_checkpoint: u64,
    pub transactions: Vec<SignedTransaction>,
    pub provisional_checkpoints: Vec<Checkpoint>,
    pub dag_edges: Vec<(String, String)>,  // (parent_hash, child_hash)
}
```

Each side sends its `PartitionDAGDelta` to the other. After exchange, both sides have the complete merged transaction set.

#### Phase 2: Conflict Detection

Scan the merged transaction set for conflicts:

**Type 1 — Direct Double-Spend:**
Same account + same nonce in both partitions. This is the classic double-spend attack.

```rust
pub struct DirectConflict {
    pub account: String,
    pub nonce: u64,
    pub tx_a: String,  // from partition A
    pub tx_b: String,  // from partition B
    pub partition_a_epoch: u64,
    pub partition_b_epoch: u64,
}
```

**Type 2 — Economic Overdraft:**
No nonce collision, but the combined spending from both partitions exceeds the account's pre-partition balance. Example: Alice had 100 RKU, spent 80 in partition A (nonce 5) and 90 in partition B (nonce 5 — this is also a Type 1). But also: Alice had 100 RKU, spent 60 in partition A (nonce 5) and 60 in partition B (nonce 6, which was valid locally because partition B didn't see nonce 5). Combined: 120 spent on 100 balance.

```rust
pub struct EconomicConflict {
    pub account: String,
    pub pre_partition_balance: f64,
    pub partition_a_total_spent: f64,
    pub partition_b_total_spent: f64,
    pub combined_deficit: f64,
    pub conflicting_txs: Vec<String>,
}
```

**Detection algorithm:**
1. Build a set of all `(account, nonce)` pairs across both partitions. Flag any duplicates as Type 1.
2. For each account that transacted in both partitions, compute:
   - `pre_partition_balance` (balance at fork point checkpoint)
   - `total_sent_A` = sum of amounts + gas fees for all txs from this account in partition A
   - `total_sent_B` = same for partition B
   - `total_received_A` = sum of amounts received in partition A
   - `total_received_B` = same for partition B
   - If `pre_partition_balance + total_received_A + total_received_B - total_sent_A - total_sent_B < 0`, flag as Type 2.

#### Phase 3: Weight Resolution

For each conflict, determine the winner using cumulative weight in the merged DAG:

**For Type 1 (Direct Double-Spend):**
- Calculate cumulative weight of both conflicting transactions across the full merged DAG
- Winner: the transaction with higher cumulative weight
- If weights are within 1.5x threshold: tiebreaker is the partition with higher total visible stake at the time of the provisional checkpoint that finalized the transaction

**For Type 2 (Economic Overdraft):**
- Order all transactions from the conflicting account by: (1) nonce ascending, (2) cumulative weight descending
- Replay transactions against the pre-partition balance in this order
- The first transaction that would cause an overdraft (and all subsequent ones from that account) are marked as losers

**Determinism:** The resolution must be deterministic — every node running the merge protocol on the same inputs must produce the same winners and losers. This is achieved by:
- Sorting transactions by hash when weights are equal
- Using pre-partition state (from the fork point checkpoint) as the canonical starting balance
- Processing accounts in deterministic order (sorted by address)

#### Phase 4: Cascade Rollback

This extends the existing `prune_losing_branch` to handle economic dependencies across the entire merged DAG.

**Current limitation:** `prune_losing_branch` only reverts DAG descendants (transactions that explicitly referenced the pruned tx as a parent). It doesn't trace **economic dependents** — transactions from accounts that received funds from the pruned transaction and subsequently spent those funds.

**Extended algorithm (using u64 micro-units for determinism):**

```
function cascade_rollback(losing_txs: Set<TxHash>, all_txs: Vec<Transaction>) -> RollbackReport:
    // Start with account balances at fork point (pre-partition) in micro-units
    balances: HashMap<Address, u64> = snapshot_balances_at_fork_point_micro()
    
    // Collect all transactions that survived conflict resolution
    surviving_txs = all_txs.filter(|tx| !losing_txs.contains(tx.hash))
    
    // Apply canonical ordering (see Section 1.1):
    //   1. Per-account nonce ascending
    //   2. DAG depth from fork-point
    //   3. Transaction hash lexicographic (deterministic tiebreaker)
    ordered_txs = canonical_sort(surviving_txs)
    
    rolled_back = Set::new()
    
    // Iterative replay — keep going until stable
    loop:
        newly_rolled_back = Set::new()
        
        // Reset balances to fork point (micro-units)
        balances = snapshot_balances_at_fork_point_micro()
        
        for tx in ordered_txs:
            if tx.hash in rolled_back:
                continue
            
            // Nonce continuity check: if this account has a nonce gap
            // (a prior nonce was rolled back), this tx is also invalid
            if tx.nonce > expected_nonce[tx.from]:
                newly_rolled_back.insert(tx.hash)
                continue
            
            sender_balance: u64 = balances[tx.from]
            amount_micro: u64 = to_micro_units(tx.amount)
            gas_micro: u64 = to_micro_units(tx.gas_price * tx.gas_limit)
            total_cost_micro: u64 = amount_micro + gas_micro
            
            if sender_balance < total_cost_micro:
                // This tx is no longer valid — the funds it needed
                // were from a transaction that got rolled back
                newly_rolled_back.insert(tx.hash)
                continue
            
            // Execute the transfer (integer arithmetic — no precision loss)
            balances[tx.from] -= total_cost_micro
            balances[tx.to] += amount_micro
            expected_nonce[tx.from] += 1
        
        if newly_rolled_back.is_empty():
            break  // Stable — no more rollbacks needed
        
        rolled_back.extend(newly_rolled_back)
    
    return RollbackReport {
        direct_conflicts: losing_txs,
        cascade_rollbacks: rolled_back - losing_txs,
        final_balances_micro: balances,
    }
```

**Important: This algorithm does NOT re-execute contract calls.** Contract balance transfers (amount + gas) are replayed, but contract storage mutations are handled separately (see Section 7.3). This ensures replay determinism regardless of contract execution environment.

**Convergence guarantee:** Each iteration can only add rollbacks, never remove them. The set of valid transactions shrinks monotonically. Since the transaction set is finite, the algorithm terminates.

**Worst case:** O(n²) where n is the number of transactions in the partition period. In practice, cascades are shallow — most transactions don't depend on funds from rolled-back transactions. A partition lasting hours with thousands of transactions might take seconds to reconcile.

#### Phase 5: State Reconciliation

After cascade rollback completes:

1. **Account State Rebuild:** The `final_balances` from the cascade rollback algorithm become the new canonical account states. These are written to `state.accounts`.

2. **DAG Cleanup:** Rolled-back transactions are either:
   - Removed from the DAG entirely (if they're double-spends — malicious)
   - Marked as `rolled_back: true` and kept in the DAG for auditability (if they're cascade victims — innocent users whose funds evaporated)

3. **Merge Checkpoint:** A new checkpoint is created that:
   - Has `provisional: false`
   - Links `previous_hash` to the fork point checkpoint (the last one both partitions agreed on)
   - Contains `finalized_tx_hashes` = all surviving (non-rolled-back) transactions from both partitions
   - Is signed by the reunified validator quorum (must meet full 2/3 threshold)
   - Contains an additional `merge_report_hash` field referencing the MergeReport
   - **Height assignment:** The merge checkpoint's height = fork_point_height + 1. All provisional checkpoint heights are discarded. This means the merge checkpoint "replaces" the provisional chains. The active checkpoint list is truncated to the fork point and the merge checkpoint is appended.

4. **Provisional Chain Retirement:** All provisional checkpoints from both partitions are archived in a separate `merge_history` store (kept for auditability) but removed from the active `state.checkpoints` list.

5. **Checkpoint Validation Bypass:** The existing `apply_checkpoint` enforces strict `height == current_height + 1` and `previous_hash` linkage. During merge:
   - The merge checkpoint uses a dedicated `apply_merge_checkpoint` code path that bypasses normal height continuity checks
   - It truncates the checkpoint chain to `fork_point_height` before appending
   - It rebuilds `state_root` and `receipt_root` from the reconciled state
   - Normal `apply_checkpoint` validation resumes for all checkpoints after the merge checkpoint

```rust
pub struct MergeCheckpoint {
    pub checkpoint: Checkpoint,  // the merge checkpoint itself
    pub fork_point_height: u64,
    pub partition_a_provisional_count: u64,
    pub partition_b_provisional_count: u64,
    pub merge_report: MergeReport,
}

pub struct MergeReport {
    pub merge_epoch: u64,
    pub fork_point_checkpoint: u64,
    pub partition_a_tx_count: u64,
    pub partition_b_tx_count: u64,
    pub direct_conflicts: Vec<DirectConflict>,
    pub economic_conflicts: Vec<EconomicConflict>,
    pub cascade_rollbacks: Vec<CascadeRollback>,
    pub penalties_assessed: Vec<PenaltyAssessment>,
    pub final_surviving_tx_count: u64,
    pub merge_timestamp: u64,
}

pub struct CascadeRollback {
    pub tx_hash: String,
    pub reason: RollbackReason,
    pub affected_account: String,
    pub amount_reverted: f64,
}

pub enum RollbackReason {
    DirectConflictLoser,
    InsufficientBalanceAfterConflictResolution,
    DependsOnRolledBackTransaction { upstream_tx: String },
}
```

---

## 6. Economic Deterrence

### 6.1 Honest vs Malicious Behavior

Distinguishing intentional double-spends from innocent partition victims:

- **Malicious:** Same account submitted transactions with the **same nonce** in both partitions. This requires the user to deliberately craft a conflicting transaction. Flag and penalize.
- **Innocent:** Account transacted normally in one partition, but received funds from a transaction that was later rolled back. They had no way to know. No penalty.
- **Gray area:** Account transacted in **both** partitions with different nonces, and the combined spending exceeds their balance. This could be innocent (user didn't know about the partition) or opportunistic. Apply a soft penalty (reputation score reduction) but no slashing.

### 6.2 Penalty Structure

```rust
pub struct PenaltyAssessment {
    pub account: String,
    pub violation_type: ViolationType,
    pub penalty_amount: f64,
    pub reputation_impact: f64,
    pub stake_slashed: f64,
}

pub enum ViolationType {
    /// Same nonce used in both partitions (definite double-spend attempt)
    NonceReuse {
        nonce: u64,
        tx_a: String,
        tx_b: String,
    },
    /// Spent in both partitions, combined exceeds balance (suspicious)
    CrossPartitionOverdraft {
        total_spent: f64,
        available_balance: f64,
    },
}
```

**Penalty schedule and formulas:**

| Violation | Balance Penalty | Reputation Impact | Stake Slash |
|-----------|-----------------|-------------------|-------------|
| Nonce reuse (definite double-spend) | `min(conflicting_amount, account_balance) * 0.10` | `reputation_penalty = 0.50` (permanent) | `staked * 1.0` (100%) |
| Cross-partition overdraft | 0 (cascade rollback handles it) | `reputation_penalty += 0.10` (decays linearly over 100 checkpoints) | None |

**Economic rationale:** For nonce reuse, the penalty must exceed the expected profit from a double-spend. The maximum profit from a double-spend is the transaction amount (you get the goods/service in one partition and keep your money in the other). A 10% balance penalty + 100% stake slash + permanent 50% weight reduction makes the expected value of a double-spend attempt strongly negative for any staked account. For unstaked accounts, the balance penalty and weight reduction still apply — their future transactions carry half weight, making them less competitive in future conflict resolutions.

**Culpability determination:** Only accounts that submitted transactions with the same nonce in different partition epochs are flagged as `NonceReuse`. The merge algorithm detects this by checking `(account, nonce)` pairs across the two `PartitionDAGDelta` sets. Economic overdrafts (different nonces, combined overspend) are treated as soft violations — the user may not have known they were in a partition.

### 6.3 Account Extension

```rust
pub struct Account {
    // ... existing fields ...
    pub address: String,
    pub balance: f64,
    pub nonce: u64,
    pub first_seen: u64,
    pub staked: f64,
    pub unbonding: f64,
    pub unbonding_release: Option<u64>,
    pub latest_balance_proof: Option<AccountStateProof>,

    // NEW: Partition-tolerance fields
    #[serde(default)]
    pub partition_violations: u32,
    #[serde(default)]
    pub reputation_penalty: f64,  // 0.0 = no penalty, 1.0 = maximum penalty
    #[serde(default)]
    pub penalty_decay_checkpoint: Option<u64>,  // checkpoint at which temporary penalties expire
}
```

### 6.4 Weight Modifier

The existing weight calculation (`age_weight * balance_weight + stake_weight`) is modified during transaction weight assignment:

```
effective_weight = base_weight * (1.0 - account.reputation_penalty)
```

This means a double-spending account's future transactions carry less weight in the DAG, making them less likely to "win" future conflicts. This is a natural extension of Rinku's existing weight-based Sybil resistance.

---

## 7. Edge Cases & Safety

### 7.1 Multi-Partition Merge

If the network splits into 3+ partitions, merges happen pairwise. The merge protocol is **commutative and associative**: merging A+B then (A+B)+C produces the same result as merging B+C then A+(B+C). This is guaranteed because:
- Conflict resolution is based on cumulative weight in the fully merged DAG
- Account state is rebuilt from the fork point each time
- Penalties are assessed based on nonce reuse detection, which is order-independent

### 7.2 Zero-Validator Partition

A partition segment with no validators:
- Can accept transactions into the DAG
- Cannot create any checkpoints (even provisional)
- Transactions accumulate as unfinalized DAG nodes
- On merge, these transactions compete purely on cumulative weight
- They are at a disadvantage because they had no checkpoint finality — but they are not automatically discarded

### 7.3 Smart Contract State Conflicts

Contracts with state mutations in both partitions require special handling:

- **Storage key conflicts:** If the same storage key is written in both partitions, the write from the winning transaction (by weight) takes precedence. This is "last-write-wins by weight."
- **Counter-style state:** Contracts that increment counters (e.g., vote counts) may lose increments from the losing partition. Contract developers should be aware of partition semantics.
- **Deterministic replay:** After cascade rollback determines the surviving transaction set, contract state is rebuilt by replaying surviving transactions in topological order against the pre-partition contract state.

### 7.4 Gas Fee Handling

- Rolled-back transactions have their gas fees returned to senders (this is already implemented in `prune_losing_branch`)
- Provisional checkpoint validators who created checkpoints with rolled-back transactions do not receive gas fees for those transactions
- The gas fee pool is recalculated from surviving transactions only

### 7.5 Long Partitions

If a partition lasts for an extended period (hours/days):
- The cascade rollback may affect a large number of transactions
- To bound the blast radius, a configurable `max_partition_epoch_duration` can be set. Beyond this, the partition creates a permanent fork (the two sides become separate networks that require manual governance intervention to reconcile)
- Default: 24 hours (configurable)

### 7.6 Nonce Gaps After Rollback

When transactions are rolled back, an account may end up with nonce gaps (e.g., nonce 5 was rolled back but nonce 6 survived because it was in the other partition). The reconciliation phase must re-sequence surviving transactions:
- After determining the surviving set, re-validate nonce continuity per account
- If gaps exist, the transactions with invalid nonces are also rolled back (they could not have been executed without the missing predecessor)
- This is handled within the cascade rollback algorithm's nonce continuity check (see Phase 4)

### 7.7 Low-Stake Partition Attack

A small partition with few validators (e.g., 1/3 of stake) can create provisional checkpoints and finalize local transactions "cheaply." An attacker could:
- Deliberately isolate a subset of validators
- Execute favorable transactions in the isolated partition
- Reconnect and hope their transactions win the weight comparison

**Mitigation:**
- Provisional checkpoints from low-stake partitions carry less credibility. The merge protocol's weight resolution uses cumulative weight across the merged DAG — transactions from a partition with more validators will naturally accumulate more weight.
- The minimum 1/3 visible stake threshold for provisional checkpoints prevents extremely small partitions from finalizing anything.
- If a partition's `visible_stake_pct < 0.50`, its provisional checkpoints are treated as "weak provisional" — they do not finalize transactions (transactions remain unfinalized). This limits the damage from deliberate partition attacks while still allowing the partition to operate.

### 7.8 Staking Rewards During Partition

Checkpoint-based emission rewards distributed during provisional checkpoints are also subject to rollback. If provisional checkpoints are retired during merge:
- Rewards allocated by those checkpoints are reverted
- The merge checkpoint allocates rewards for the surviving transaction set only
- Validators in both partitions receive proportional rewards based on the surviving checkpoints

---

## 8. Data Structure Changes Summary

### New Structs

| Struct | Location | Purpose |
|--------|----------|---------|
| `PartitionState` | `state/partition.rs` (new) | Tracks current partition status, epoch, visible validators |
| `PartitionDetector` | `partition.rs` (new) | Background service monitoring network health |
| `MergeReport` | `state/merge.rs` (new) | Complete record of a partition merge |
| `MergeCheckpoint` | `state/merge.rs` (new) | Merge checkpoint with associated report |
| `DirectConflict` | `state/merge.rs` (new) | Type 1 conflict record |
| `EconomicConflict` | `state/merge.rs` (new) | Type 2 conflict record |
| `CascadeRollback` | `state/merge.rs` (new) | Individual rollback record |
| `PenaltyAssessment` | `state/merge.rs` (new) | Penalty applied to an account |
| `PartitionDAGDelta` | `state/merge.rs` (new) | DAG state exchanged during merge |

### Modified Structs

| Struct | New Fields | Notes |
|--------|-----------|-------|
| `Checkpoint` | `provisional: bool`, `partition_epoch: Option<u64>`, `visible_stake_pct: Option<f64>` | Backward compatible via `#[serde(default)]` |
| `DagNode` | `partition_epoch: Option<u64>`, `provisional_finality: bool` | Backward compatible |
| `Account` | `partition_violations: u32`, `reputation_penalty: f64`, `penalty_decay_checkpoint: Option<u64>` | Backward compatible |
| `InnerState` | `partition_state: PartitionState` | Internal state only |

### New Modules

| Module | Purpose |
|--------|---------|
| `state/partition.rs` | Partition state management |
| `state/merge.rs` | Merge protocol implementation |
| `partition_detector.rs` | Background partition detection service |

---

## 9. API & WebSocket Changes

### New HTTP Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/network/partition` | GET | Current partition status, epoch, visible validators, stake percentage |
| `/api/partition/merge` | POST | Trigger manual merge (testing/governance). Accepts `{ peer_url: String }` |
| `/api/merge/report/{epoch}` | GET | Returns MergeReport for a completed partition merge |
| `/api/merge/history` | GET | List of all completed merges with summary stats |

### New WebSocket Events

```typescript
type PartitionEvent =
  | { type: "PartitionSuspected", visibleStakePct: number, missingValidators: string[] }
  | { type: "PartitionConfirmed", epoch: number, visibleValidators: string[] }
  | { type: "PartitionHealing", reconnectedPeers: string[] }
  | { type: "MergeStarted", epoch: number, peerPartitionEpoch: number }
  | { type: "MergeProgress", phase: string, detail: string }
  | { type: "MergeCompleted", report: MergeReportSummary }
  | { type: "TransactionRolledBack", txHash: string, reason: string }
  | { type: "PenaltyAssessed", account: string, violationType: string, amount: number }
```

### Explorer UI Additions

- **Partition Status Banner:** Shown in the header when `status != Normal`. Displays epoch, visible stake %, and list of unreachable validators.
- **Provisional Finality Indicator:** Transactions finalized during a partition show a yellow "provisional" badge instead of green "confirmed."
- **Merge History Tab:** Shows past merge events with expandable reports (conflicts found, rollbacks, penalties).
- **Account Reputation:** Account pages display `partition_violations` and current `reputation_penalty` if non-zero.

---

## 10. Implementation Phases

### Phase 1: Partition Detection + Provisional Checkpoints
**Scope:** Detect partitions, enter/exit partition mode, create provisional checkpoints.
**Extends:** `checkpoint.rs`, `gossip.rs`, `state/mod.rs`
**New:** `partition_detector.rs`, `state/partition.rs`
**Complexity:** Medium — mostly new code, minimal changes to existing hot paths

### Phase 2: Merge Protocol + Conflict Detection
**Scope:** DAG exchange, Type 1 and Type 2 conflict detection, weight-based resolution.
**Extends:** `gossip.rs` (handshake), `state/sync.rs` (DAG exchange)
**New:** `state/merge.rs`
**Complexity:** High — the conflict detection across two DAGs is the most algorithmically complex piece

### Phase 3: Cascade Rollback Algorithm
**Scope:** Extended rollback that traces economic dependencies, iterative replay until convergence.
**Extends:** `state/fork.rs` (extend `prune_losing_branch`)
**New:** Cascade rollback function in `state/merge.rs`
**Complexity:** High — must handle arbitrary transaction dependency chains and guarantee convergence

### Phase 4: Economic Deterrence + Reputation
**Scope:** Nonce-reuse detection across partitions, penalty assessment, weight modifier.
**Extends:** `Account` struct, weight calculation in `rinku-core/src/weight.rs`, `fork_remediation.rs`
**Complexity:** Low-Medium — straightforward penalty logic on top of existing structures

### Phase 5: Explorer UI + API
**Scope:** New API endpoints, WebSocket events, explorer components for partition status and merge history.
**Extends:** `api.rs`, `events.rs`, `websocket.rs`, explorer components
**Complexity:** Medium — standard API/UI work

### Dependency Graph

```
Phase 1 ──► Phase 2 ──► Phase 3 ──► Phase 4
                                        │
                                        ▼
                                     Phase 5
```

Phase 1 must come first (partition detection is prerequisite for everything). Phases 2-3 are tightly coupled (merge needs rollback). Phase 4 can be done after rollback works. Phase 5 can start in parallel with Phase 4 for the API portions.

---

## 11. Testing Strategy

### Unit Tests
- Partition detection state machine transitions
- Conflict detection on synthetic DAGs with known double-spends
- Cascade rollback convergence on crafted dependency chains
- Penalty calculation correctness

### Integration Tests
- Two-node partition simulation: disconnect nodes, transact independently, reconnect, verify merge
- Double-spend across partitions: craft same-nonce transactions, verify correct resolution
- Cascade scenario: A→B→C fund chain, roll back A, verify B and C are also rolled back
- Multi-partition merge: three nodes in three partitions, merge pairwise, verify consistency

### Stress Tests
- Long partition (1000+ transactions per partition), measure merge time
- Deep cascade chains (10+ levels), verify convergence
- Rapid partition/heal cycles, verify state consistency

---

## 12. Comparison to Prior Art

| Feature | SwarmDAG | Vegvisir | Rinku (proposed) |
|---------|----------|----------|------------------|
| Partition detection | EVS membership | CRDT-based | Stake-visibility heuristic |
| During partition | Independent consensus | Append-only log | Provisional checkpoints |
| Merge strategy | DAG fork preservation | CRDT merge | Weight-based conflict resolution + cascade rollback |
| Double-spend prevention | Not addressed (no financial layer) | Not addressed | Cumulative weight + economic deterrence |
| Smart contracts | No | No | Yes (WASM, state conflict resolution) |
| Financial layer | No | No | Yes (hard-capped token, gas fees, staking) |
| Penalty mechanism | No | No | Yes (slashing, reputation scoring) |
