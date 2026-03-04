# Rinku: A Distributed Ledger For Mesh-Native Systems

###### [rinkuchan.com](https://rinkuchan.com)


## Abstract

The next wave of distributed computing is not data centers - it is autonomous agents: drone swarms, robotic fleets, and mobile sensor networks operating on adhoc mesh infrastructure where connectivity is intermittent by design, not an edge case. Existing distributed ledgers are structurally insufficient for this environment; they halt when the network partitions, require persistent RPC infrastructure for state verification, or their smart contracts assume a synchronized, always-online world.

Rinku is a DAG-based distributed ledger built around three primitives designed for exactly this environment. Tunable consistency allows the protocol to navigate the CAP tradeoff dynamically: delivering CP-like checkpoint finality when quorum is reachable, degrading gracefully to provisional availability during partitions, and deterministically reconciling state when connectivity is restored - ensuring that a swarm of robots on a local mesh can transact continuously and settle correctly when they reconnect to the broader network. VerifiableObjects are self-contained, URL-encoded cryptographic proofs that carry everything needed for offline verification - no full node, no RPC endpoint, no network access. An autonomous agent can receive payment, verify it locally with a single BLS check, and execute its task before ever touching external infrastructure. BYOP (Bring Your Own Proof) smart contracts accept VerifiableObjects as inputs, enabling contract logic to execute against proven external state without synchronous cross-chain or cross-contract calls - service terms, payment conditions, and execution receipts composable without centralized coordination.

Together, these three primitives describe a ledger that does not merely tolerate the mesh-native economy - it is designed for it. The native RKU token has a hard cap of 30 million units with checkpoint-based emission, weighted proof-of-stake rewards, and an adaptive fee burn mechanism.

---

## Table of Contents

1. [Introduction](#1-introduction)
2. [Trust Model & Terminology](#2-trust-model)
3. [VerifiableObject System](#3-verifiable-objects)
4. [Design Philosophy](#4-design-philosophy)
5. [Network Architecture](#5-network-architecture)
6. [Consensus: Fast Acceptance & Checkpoint Finality](#6-consensus)
7. [DAG Structure & Transaction Ordering](#7-dag-structure)
8. [Partition Tolerance](#8-partition-tolerance)
9. [Reconciliation Semantics & Transaction Taxonomy](#9-reconciliation)
10. [Smart Contracts & WASM Runtime](#10-smart-contracts)
11. [Economic Model](#11-economic-model)
12. [Slashing & Economic Security](#12-slashing)
13. [Privacy Layer](#13-privacy)
14. [Networking & P2P Protocol](#14-networking)
15. [Future Work](#15-future-work)
16. [Conclusion](#16-conclusion)

- [References](#references)

---

## 1. Introduction

Distributed ledgers have achieved remarkable success in environments where network connectivity is reliable and persistent. Bitcoin's Nakamoto consensus and Ethereum's Gasper protocol deliver strong probabilistic or deterministic finality under the assumption that a sufficient fraction of participants can communicate within reasonable time. BFT-family protocols - Tendermint, HotStuff, Bullshark, Mysticeti - push finality latency down to seconds or sub-seconds, but all share a fundamental constraint: they halt when the network cannot reach quorum.

Admittedly, this constraint is acceptable for data-center-class infrastructure where partitions are rare & short-lived. However it is unacceptable for mesh-native or fragmented environments; ad-hoc wireless networks, mobile-first deployments in regions with less than consistent connectivity; any setting where partitions are expected operating conditions rather than rare edge cases. Furthermore, these environments shouldn't be disallowed the same economic primatives that the others readily support.

Thankfully, there is no shortage of academic literature that offer rich solutions for partition tolerance - CRDTs, eventual consistency, Bayou-style conflict resolution, anti-entropy protocols; but these techniques have seen limited adoption in production blockchain design. The gap exists because financial state is not naturally commutative: transferring tokens is an inherently non-idempotent operation where order matters and conflicts have economic consequences.

Rinku bridges this gap. It provides a distributed ledger designed for environments where partitions are a first-class operating condition, not a side-effect to be circumnavigated. The protocol maintains strong finality when the network is connected and degrades gracefully to provisional operation during partitions, with a deterministic merge protocol that reconciles state when connectivity is restored.

### 1.1 The Partition Problem

Traditional blockchains prioritize a single canonical history with strong finality but sacrifice availability - the network halts when partitions prevent quorum. There are typically 3 categories for how these halts are handled:

Category 1 (Nakamoto): Live during partition, reorg losers on reconnect, with no intelligent reconciliation
Category 2 (BFT): Halt finalization, queue in mempool; safety over liveness
Category 3 (Hybrid/Ethereum): Tips keep moving, finality pauses; halfway measure

So far, these have proven fairly sufficient for strongly connected infrastructure, but for networks that are disjoined or regularly fragmented there still remains to be a more appropriate solution.

### 1.2 Rinku's Position

Rinku takes a different position: **tunable consistency**. When the network is fully connected, Mysticeti-FPC delivers sub-second transaction acceptance and checkpoint-based settlement finality. When partitions occur, the network degrades gracefully to provisional acceptance - transactions continue locally, and state reconciles deterministically when partitions heal. This ensures zero disruption to availability, while equally retaining consistency and concensus.

The core invariant: **no honest user is prevented from transacting during a partition.** Naturally, the cost of this guarantee is that some transactions may be rolled back during merge if they conflict with transactions from other partitions. Intentional abuse is economically penalized.

### 1.3 VerifiableObjects as the User-Facing Primitive

Rinku's most distinctive user-facing concept is the **VerifiableObject** (VO): a self-contained, URL-encoded cryptographic proof that carries all data necessary for offline verification. Every meaningful output of the protocol - a payment confirmation, a contract execution receipt, a trust score attestation - is expressible as a portable `rinku://vo/` URL. This collapses the distinction between "querying the chain" and "holding a proof" - any party with a VO can verify its claim without a full node, an RPC endpoint, or any network access at all. Section 3 expands on this design much further.

---

## 2. Trust Model & Terminology {#2-trust-model}

Before describing the protocol, we establish the trust assumptions and define key terms precisely. These definitions apply throughout the paper.

### 2.1 Genesis Trust Anchor

The Rinku network is bootstrapped from a **genesis checkpoint** signed by an initial validator set defined in node configuration (`GENESIS_VALIDATORS`). This genesis checkpoint establishes:

- The initial account state (genesis allocation of 6,000,000 RKU)
- The founding validator set with initial stakes
- The root of the checkpoint hash chain

All subsequent state is cryptographically derived from this anchor. Non-genesis nodes verify the genesis validator set against their own configuration and will wipe and resync if the genesis state is inconsistent.

### 2.2 Validator Set Evolution

The validator set evolves through staking and unstaking transactions on the ledger:

**Joining the validator set.** Any account may become a validator by submitting a `Stake` transaction locking at least 100 RKU. Upon finalization (inclusion in a checkpoint signed by the current quorum), the staking account is added to the active validator set maintained by the `ValidatorIdentityService`. The new validator's BLS public key is registered and becomes eligible for checkpoint voting at the next height.

**Leaving the validator set.** A validator submits an `Unstake` transaction. The stake enters a 24-hour cooldown period during which it remains locked and slashable but no longer earns rewards. After cooldown, the stake is returned to the liquid balance. A 14-day unbonding window applies for slashing purposes - evidence of misbehavior during the bonded period can still trigger slashing during unbonding.

**Checkpoint signing.** Each checkpoint is signed by the validators active at that height. The `ConsensusService` maintains a frozen snapshot of the validator set for each voting round, ensuring that staking changes mid-round do not affect quorum calculation. New validators participate in signing starting from the checkpoint following their stake finalization.

**Genesis-to-runtime transition.** The genesis node creates the initial validator accounts and registers their stakes. Non-genesis nodes bootstrap by syncing the genesis checkpoint and adopting the genesis validator set from `GENESIS_VALIDATORS` configuration. During sync, if a node detects that its local genesis state is inconsistent with the peer's, it performs a full database wipe and resync to ensure all nodes share the same trust root. The runtime validator set evolves independently from the genesis set as staking transactions are processed.

### 2.3 Terminology: Acceptance vs. Finality

This paper uses precise terminology for transaction lifecycle states:

| Term | Meaning | Guarantee |
|------|---------|-----------|
| **Submitted** | Transaction received by a node and inserted into the DAG | No durability guarantee |
| **Accepted** | Transaction has received fast-path acknowledgment from >2/3 active stake | High confidence of inclusion; pre-checkpoint; not yet durable |
| **Finalized** | Transaction referenced by a checkpoint signed by >2/3 total stake quorum | Irreversible under honest majority assumption |
| **Provisionally finalized** | Transaction referenced by a provisional checkpoint during partition mode | Valid within the local partition; subject to rollback during merge |
| **Reconciled** | Previously provisional transaction that survived the merge protocol | Equivalent to finalized; referenced by the merge checkpoint |
| **Rolled back** | Previously provisional transaction that was rejected during merge | Removed from canonical state; kept in DAG for auditability if cascade victim |

### 2.4 What Receipts Prove

| Receipt Type | Mode | Anchor | Proves |
|-------------|------|--------|--------|
| **Fast-path ACK** | Normal | Validator stake votes | >2/3 stake has seen and accepted the transaction. Does not prove checkpoint inclusion. |
| **Checkpoint receipt** | Normal | BLS-signed checkpoint with state root | Transaction is included in a finalized checkpoint. Irreversible. |
| **Provisional receipt** | Partition | Provisional checkpoint with `partition_epoch` | Transaction is locally finalized within this partition. Subject to rollback. |
| **Reconciliation receipt** | Post-merge | Merge checkpoint with `merge_report_hash` | Transaction survived reconciliation and is now globally finalized. |
| **Rollback receipt** | Post-merge | Merge report | Transaction was rolled back, with reason (conflict loser, cascade victim). |

### 2.5 Threat Model

Rinku's security properties rely on the following assumptions:

**Honest majority.** The protocol assumes that validators controlling more than 2/3 of total staked RKU follow the protocol honestly. This is the standard BFT assumption. Under this assumption, finalized checkpoints are irreversible - a conflicting checkpoint would require >1/3 stake to sign two different checkpoint hashes at the same height, which is detected and slashed (section 12).

**Byzantine fault tolerance.** Up to f < n/3 validators (by stake weight) may behave arbitrarily - equivocating, withholding votes, or broadcasting invalid messages. The protocol guarantees safety (no conflicting finalized checkpoints) as long as the honest majority assumption holds. Liveness requires >2/3 stake to be reachable for checkpoint finality; during partitions, liveness is maintained locally through provisional checkpoints (Section 8).

**Partition attacker model.** An adversary capable of partitioning the network can:

- Cause some partitions to operate under provisional finality (reduced but functional)
- Cause some transactions to be rolled back during merge if they conflict across partitions
- Trigger cascade rollbacks that affect innocent users whose transactions depended on rolled-back funds

An adversary **cannot**:

- Forge transactions (ECDSA P-256 signatures)
- Cause conflicting finalized (non-provisional) checkpoints (requires >2/3 honest stake)
- Double-spend without detection and penalty (nonce reuse is detected during merge; Section 12)
- Exploit partition tolerance for profit without incurring economic penalties that exceed the potential gain (Section 12.4)

**Sybil resistance.** Identity and influence in the protocol are derived from staked RKU. Weight calculations use sub-linear stake scaling (`stake^0.5 * 2.0`) to reduce the advantage of large stakers while maintaining Sybil resistance. Creating many low-stake validators provides diminishing returns compared to a single high-stake validator.

---

## 3. VerifiableObject System {#3-verifiable-objects}

VerifiableObjects (VOs) are Rinku's universal container for portable, self-proving cryptographic claims. Every proof type in the system produces a `rinku://vo/` URL with embedded proof data and freshness metadata. VOs are the primary interface between the Rinku protocol and external consumers - they are how the ledger communicates provable facts to the world. Furthermore, the resource allocation typically required for the standard verification via extraneous resources (i.e running a node), or API / reliable network connectivity to network validation is no longer necessary. This could prove exceptionally powerful for lightweight operators, such as drones.

### 3.1 Proof Types

| Type | Description | Use Case |
|------|-------------|----------|
| **ContractOutput** | StatefulReceipt with view keys, pre/post state roots, events | Stateless dApp verification |
| **AccountProof** | Balance, nonce, stake at a specific checkpoint | Account state verification |
| **TxFinality** | Transaction inclusion proof with Merkle path and BLS signature | Payment confirmation |
| **WeightProof** | Aggregate stake weight attestation | Trust scoring, anti-disinformation |
| **BatchProof** | Multi-receipt verification with shared checkpoint context | Bulk verification |
| **StateWitness** | Sparse Merkle multiproof for contract storage keys | Stateless contract reads |
| **Custom** | Schema-defined proofs for application-specific claims | Extensibility |

### 3.2 URL Encoding

VOs are serialized to JSON, DEFLATE-compressed, and encoded as URL-safe Base64:

```
rinku://vo/<base64_compressed_json>
```

Additional URI schemes for specific proof types:

- `rinku://sp/` - Self-contained proofs (account state with full Merkle path to checkpoint)
- `rinku://asp/` - Account state proofs (compact)

The URL encoding is designed so that a VO can be shared as a hyperlink, embedded in a QR code, or passed as a transaction parameter - collapsing the boundary between "data" and "proof of data."

### 3.3 Proof Freshness

Every VO carries optional `ProofFreshness` metadata:

- `generated_at_checkpoint` - checkpoint height at proof generation
- `generated_at_timestamp` - wall-clock time of generation
- `max_age_checkpoints` - optional expiry window

Verifiers can enforce proof age limits, preventing the use of stale proofs in time-sensitive operations. This is critical for BYOP transactions (Section 3.4) where a contract must ensure it is acting on recent state.

### 3.4 BYOP (Bring Your Own Proof) Transactions

Contracts accept `ProofInput` arrays as transaction parameters. Each proof input carries a `VerifiableObject` and a `ProofExpectation` specifying:

- Required proof type
- Chain ID (for potential cross-chain use)
- Minimum checkpoint height
- Expected state root

The runtime validates all proofs before contract execution begins and injects verified data into the WASM context under the `proof.<label>.<key>` namespace. This enables contracts to act as their own oracles - consuming proven facts from other contracts, accounts, or even external chains without synchronous state access.

**Security against proof replay.** Proof freshness requirements mitigate replay attacks where a valid but stale proof is resubmitted to a contract. The `ProofExpectation` includes a `max_age_checkpoints` field; the runtime computes the proof's age as `current_checkpoint_height - generated_at_checkpoint` and rejects proofs that exceed the caller's specified age limit. This enforcement occurs at the verifier's constraint level, not the proof's own `max_age_checkpoints` - ensuring that the consumer of a proof, not its producer, controls freshness requirements.

**Cross-contract receipt composition.** A contract can consume the `StatefulReceipt` of another contract's execution as a `ProofInput`. For example, a lending contract can accept a price oracle contract's receipt as proof of the current collateral value, without making a synchronous cross-contract call. The lending contract validates the oracle receipt's Merkle proof against the state root, checks freshness, and proceeds with the proven value. This pattern enables composability without re-entrancy risk.

---

## 4. Design Philosophy {#4-design-philosophy}

### 4.1 URL-Native State

Rinku embeds the entire ledger state within cryptographically-linked URLs. Every proof, receipt, and state witness can be encoded as a self-contained URL that carries all data necessary for offline verification. This eliminates the dependency on full-node infrastructure for trust - any party holding a Rinku URL can independently verify its claim against a checkpoint anchor.

### 4.2 Self-Provable Ledger

Rinku's proof architecture differs fundamentally from SPV (Simplified Payment Verification) and traditional light client models:

**SPV proofs** (Bitcoin) prove transaction inclusion in a block via Merkle path but require the verifier to trust block headers, which in turn requires following the longest-chain rule. The verifier must either maintain a header chain or trust a third party that does. SPV proofs do not prove account state - only transaction inclusion.

**Light clients** (Ethereum Beacon Chain, Cosmos IBC) verify state transitions by tracking a committee of validators and their signatures. They require an ongoing connection to at least one honest full node to follow the validator set evolution. Without this connection, a light client's view becomes stale and unverifiable.

**Rinku self-contained proofs** carry everything needed for offline verification in a single URL:

1. The claim itself (account balance, transaction finality, contract output)
2. A Merkle inclusion proof connecting the claim to a checkpoint state root
3. The BLS aggregate signature over the checkpoint hash
4. A signer bitmap identifying which validators signed
5. Signer membership proofs (Merkle Sum Tree) proving each signer was a member of the validator set with their claimed stake weight

A verifier holding a self-contained proof and a trusted checkpoint (or the genesis checkpoint) can verify the claim without any network access, any RPC endpoint, or any trust in the proof's provider. The proof is self-authenticating - either the math checks out or it doesn't.

This removes the reliance of "blockchain infrastructure" for read operations. Where Ethereum requires archive nodes, RPC providers (Infura, Alchemy), and client libraries to answer "what is Alice's balance?", Rinku answers the same question with a URL that anyone can verify with a quick, single cryptographic check.

### 4.3 Tunable Consistency

Rinku does not claim to "solve" or "beat" the CAP theorem. Instead, it navigates CAP by dynamically selecting the appropriate tradeoff:

- **Normal operation:** CP-like strong finality via checkpoint quorum. Transactions achieve irreversible settlement through BLS-signed checkpoints backed by >2/3 total stake. This is not classical linearizability in the distributed systems sense, but provides the practical guarantees users expect: once finalized, a transaction cannot be reverted.
- **Partition mode:** Provisional availability. Transactions continue locally with explicitly labeled provisional finality. Clients are informed that these confirmations are subject to rollback.
- **Post-partition:** Deterministic convergence to global consistency through the merge protocol (Section 9). All nodes independently compute identical reconciled state from the same inputs.

The innovation is not the mode switching itself - it is the **transaction taxonomy and reconciliation semantics** that make this mode switching practical for financial state without unacceptable rollback risk.

### 4.4 CAP Analysis

Formally, Rinku's position in the CAP design space is:

| Mode | Consistency | Availability | Partition Tolerance |
|------|------------|-------------|-------------------|
| Normal | Strong (checkpoint finality) | Full (fast-path + checkpoint) | N/A (no partition) |
| Partitioned | Provisional (local consistency only) | Full (all Safe/BoundedSpend operations) | Full |
| Post-merge | Strong (deterministic reconciliation) | Temporarily reduced (merge computation) | N/A (partition healed) |

During partition, Rinku explicitly sacrifices global consistency in favor of local availability. The key insight is that this sacrifice is **bounded and recoverable**: the transaction taxonomy (Section 9.8) limits which operations can proceed, the partition budget system bounds the economic exposure, and the merge protocol deterministically recovers global consistency.

This is not eventual consistency in the CRDT sense - Rinku does not guarantee that all operations commute. Instead, it guarantees that non-commutative operations (financial transfers) are either merge-safe by design (within partition budget) or subject to explicit, deterministic conflict resolution with graduated economic penalties for abuse.

---

## 5. Network Architecture {#5-network-architecture}

### 5.1 Peer-to-Peer Layer

Rinku uses pure libp2p with gossipsub for all inter-node communication. There are no HTTP-based node-to-node calls. The protocol operates on two channels:

- **Gossip topic (`rinku/1.0.0`):** Transaction broadcast, checkpoint announcements, tip announcements, merge payloads
- **Request-response protocol (`/rinku/sync/1.0.0`):** Checkpoint sync, delta sync, snapshot recovery, presync, partition visibility queries

Note: 1.0.0 reflects current protocol version.

### 5.2 Node Roles

**Validator nodes** participate in consensus by staking RKU, voting on checkpoints, producing fast-path ACKs, and proposing checkpoints when elected as leader. Validators run the full protocol stack including the partition detector, merge orchestrator, and tip consolidation service. Only the genesis node creates the initial validator accounts and registers their stakes; non-genesis validators adopt the validator set during sync.

**Full nodes** maintain a complete copy of the DAG and state but do not participate in checkpoint voting. They validate all transactions and checkpoints, serve API requests, relay gossip messages, and can independently verify the entire chain from genesis. Full nodes contribute to network resilience by increasing the number of honest relays.

**Light clients (TBA..?).** The self-contained proof architecture (Section 4.2) enables a lightweight verification mode where clients hold no ledger state. A light client can verify any claim by receiving a `rinku://sp/` URL and checking the BLS signature, Merkle proof, and signer membership proofs against a trusted checkpoint anchor. This model requires no ongoing connection to any full node - verification is a single, stateless cryptographic operation.

### 5.3 Synchronization

Rinku implements a multi-mode synchronization system optimized for different scenarios:

**Snapshot sync** (new nodes). A joining node requests a complete state snapshot from a peer. The snapshot includes all account balances, stakes, contract state, and the current checkpoint chain. After applying the snapshot, the node replays any transactions in the DAG that occurred after the snapshot's checkpoint height. Transactions are only marked `finalized: true` if they appear in the snapshot's `finalized_tx_hashes` or in checkpoint `finalized_tx_hashes` lists - preventing spurious finality attribution during recovery.

**Delta sync** (catching up). A node that has been briefly disconnected requests only the transactions and checkpoints created since its last known checkpoint. This is a lightweight operation that avoids retransmitting the full state. Delta sync also reconciles the validator identity service, ensuring the catching-up node's `total_active_stake` matches the network.

**Presync** (quick bootstrap). Before performing a full snapshot sync, a joining node performs a lightweight handshake to determine the peer's checkpoint height and validator set. This allows the node to estimate the sync workload and select the most appropriate sync mode.

**Persistent storage.** All state is persisted using `redb`, a lightweight embedded database. The persistent transaction counter (`total_transactions`) is stored in metadata alongside `gas_price`, `total_supply`, and `genesis_time`, ensuring accurate counts survive DAG pruning across restarts.

**Ghost account prevention.** During sync, account push-back filters out stale accounts (zero balance, zero nonce) to prevent state contamination from obsolete data.

---

## 6. Consensus: Fast Acceptance & Checkpoint Finality {#6-consensus}

Rinku implements a dual-layer confirmation model: fast-path acceptance for sub-second confirmation, and checkpoint-based finality for strong, provable settlement.

### 6.1 Fast-Path Acceptance

Fast-path provides sub-second (100–500ms) transaction **acceptance** through stake-weighted validator voting:

- Validators receive transactions via gossip and broadcast ACK votes
- A transaction is fast-path accepted when accumulated ACK stake exceeds 2/3 of total active stake
- Fast-path acceptance enables immediate UX feedback; it represents high-confidence inclusion but is not equivalent to checkpoint finality

Fast-path acceptance is a **confirmation** signal, not a **finality** guarantee. It tells the client: "the validator majority has seen your transaction and intends to include it." Durable, irreversible finality comes from checkpoint settlement.

All transaction types are eligible for the fast path. When a validator receives a transaction and validates its signature, nonce, and balance, it broadcasts a `FastPathAck` containing its stake weight and BLS signature over the transaction hash. The originating node accumulates ACKs until the 66.7% quorum threshold is met (`FAST_PATH_QUORUM_THRESHOLD`), at which point the transaction is marked `Confirmed` and can be executed optimistically.

To prevent double-execution, fast-path-executed transactions are tracked in a `fast_path_executed` set and skipped during the subsequent checkpoint finalization phase.

### 6.2 Checkpoint Finality

Checkpoints anchor durable, irreversible finality at regular intervals (~10 seconds):

- A leader is elected per checkpoint height
- The leader proposes a checkpoint including: transaction Merkle root, state root (from Sparse Merkle Trie), receipt root, BLS aggregate signature
- Validators vote on the checkpoint; 2/3 total stake quorum required for finalization
- Once finalized, all transactions referenced by the checkpoint receive **settlement finality** - they cannot be reverted under the honest majority assumption

The checkpoint creation flow:

1. The elected leader collects all unfinalized transactions from its local DAG.
2. Transactions are filtered using a `PROPAGATION_GRACE_MS` window (default 5 seconds) to ensure sufficient time for gossip propagation - this increases the likelihood that other validators have received the same transaction set.
3. The leader computes the `tx_merkle_root` over the filtered transaction hashes and the `state_root` from the Sparse Merkle Trie.
4. The leader broadcasts the `Checkpoint` via a `CheckpointAnnouncement` gossip message (which includes transaction bodies to prevent balance divergence).
5. Non-leader validators receive the checkpoint, compare the `tx_merkle_root` against their own unfinalized transactions, and either adopt the checkpoint (if roots match) or enter a sync phase to reconcile missing transactions.

### 6.3 BLS Aggregate Signatures

Rinku uses the BLS12-381 signature scheme (via the `blst` library, `min_pk` variant) for checkpoint signing:

**Signature scheme.** Signatures are on the G2 group (96 bytes compressed); public keys are on G1 (48 bytes compressed). Each validator signs the checkpoint hash with their BLS secret key, producing an individual signature.

**Aggregation.** Individual validator signatures are aggregated into a single 96-byte aggregate signature using `AggregateSignature::aggregate`. This aggregation is additive - the aggregate signature can be verified against the aggregate of the corresponding public keys in a single pairing check, regardless of the number of signers.

**Signer bitmaps.** To identify which validators contributed to an aggregate signature without transmitting the full validator list, Rinku uses a compact bitfield. The `signer_bitmap` is a `Vec<u8>` where the *i*-th bit corresponds to the *i*-th validator in the deterministically sorted validator set. A set bit indicates that validator signed the checkpoint. This enables any verifier with knowledge of the validator set to reconstruct the aggregate public key and verify the aggregate signature.

**Verification.** Given a checkpoint hash, aggregate signature, signer bitmap, and validator set, verification proceeds: (1) parse the bitmap to identify signers, (2) aggregate their public keys, (3) verify the aggregate signature against the aggregate public key and checkpoint hash. This is a constant-time operation regardless of the number of signers (a single pairing check).

**Double-sign detection.** The `ConsensusService` monitors for validators that sign two different checkpoint hashes at the same height. Double-signing triggers an immediate 15% stake slash and addition to a `slashed_validators` set, which reduces the validator's voting power in all pending rounds.

### 6.4 Leader Election

Rinku uses a deterministic, stake-weighted leader election mechanism based on verifiable randomness:

**Randomness derivation.** For each checkpoint height, a 32-byte seed is computed as:

```
randomness = SHA-256("RINKU_LEADER_ELECTION_V1" || checkpoint_height || previous_checkpoint_hash)
```

Since all validators in consensus share the same `previous_checkpoint_hash` and target height, they all derive identical randomness. The seed is unpredictable before the previous checkpoint is finalized (depends on the previous checkpoint's hash) but deterministic afterward.

**Stake-weighted selection.** The active validator set is sorted by address (ensuring identical ordering on all nodes). The randomness is mapped to a target value in the range `[0, total_stake)`. The algorithm iterates through the sorted validators, accumulating stake until the cumulative sum exceeds the target. The validator that crosses the threshold is elected leader. Over time, each validator's probability of election is proportional to their stake.

**Rotation.** Because `checkpoint_height` is an input to the randomness, the leader changes every checkpoint. The combination of height-based rotation and stake-weighted selection ensures both fairness (proportional to stake) and unpredictability (cannot be known until the previous checkpoint finalizes).

**Liveness fallback.** If the elected leader fails to produce a checkpoint within a configurable timeout (`leader_timeout_ms`, default 45 seconds), a fallback mechanism activates. The `should_fallback` function uses a modified height input (`checkpoint_height + 1000000`) for randomness, effectively electing an emergency replacement leader. This ensures liveness even if the primary leader is offline.

### 6.5 Relationship Between Acceptance and Finality

| Property | Fast-Path Acceptance | Checkpoint Finality |
|----------|---------------------|-------------------|
| Latency | 100–500ms | ~10s |
| Quorum | >2/3 active stake (ACK votes) | >2/3 total stake (checkpoint signatures) |
| Durability | Not persisted; lost on restart | Persisted; survives restart and sync |
| Irreversibility | Can theoretically be excluded from checkpoint | Irreversible under honest majority |
| Proof artifact | Fast-path ACK set | BLS-signed checkpoint with state/receipt roots |

---

## 7. DAG Structure & Transaction Ordering {#7-dag-structure}

### 7.1 Transaction DAG

Transactions in Rinku form a DAG rather than a linear chain. Each transaction references one or more parent transactions (DAG tips at the time of submission), creating a partial order that enables parallel processing.

### 7.2 Tip Selection

Rinku uses a **Sparse DAG Sampling** algorithm to prevent tip explosion while maintaining DAG connectivity and Sybil resistance.

**MAX_SAMPLED_TIPS.** The maximum number of parent references a transaction selects is bounded at 16. This prevents the DAG from growing excessively wide while maintaining sufficient connectivity for parallel validation.

**Weighted selection with diversity.** When the number of available tips exceeds 16, the sampling algorithm splits selection into two halves:

1. **Guaranteed selection (top 8):** The 8 tips with the highest sender account weight are always included. This ensures that well-staked, high-reputation transactions are preferentially referenced, providing Sybil resistance - an attacker flooding the network with low-weight transactions cannot crowd out legitimate tips.
2. **Random sampling (bottom 8):** The remaining 8 slots are filled by random sampling from all other available tips. This maintains DAG diversity, prevents the graph from narrowing to a single chain of high-weight transactions, and ensures that transactions from lower-weight (but honest) participants are eventually incorporated.

**Tip consolidation.** A background `TipConsolidator` service runs on validator nodes. When the tip count exceeds a threshold (default 100), it enters aggressive consolidation mode, periodically creating `Consolidation` transactions that reference 16 tips at once. These anchor transactions merge divergent branches back into fewer tips, keeping the DAG's working set manageable.

**Orphan parent handling.** If a transaction arrives with parent references to hashes not found in the local DAG (orphan parents), the node automatically injects current known tips as parents. This ensures the transaction attaches to the main graph even if some referenced parents were pruned or never received.

### 7.3 Weight Calculation

Transaction weight is computed as:

```
effective_weight = (age_weight * balance_weight + stake_weight) * (1.0 - reputation_penalty)
```

Where:

- **age_weight:** Time-based component since transaction insertion
- **balance_weight:** Derived from the sender's account balance
- **stake_weight:** Sub-linear bonus from staked tokens, computed as `stake^0.5 * 2.0`. The square-root scaling reduces the advantage of large stakers - doubling stake increases weight by only ~41%, not 100%. This provides meaningful Sybil resistance while limiting plutocratic concentration.
- **reputation_penalty:** A value in `[0.0, 1.0]` reflecting accumulated partition-tolerance violations (see Section 12). An account with a 0.50 reputation penalty has its effective weight halved in all weight calculations.

The sub-linear stake weight is a deliberate design choice for decentralization: it makes it more capital-efficient to distribute stake across multiple honest validators than to concentrate it in a single large validator, while still ensuring that staked participants have significantly more influence than unstaked ones.

### 7.4 Cumulative Weight & Conflict Resolution

**Intra-partition fork resolution.** Within a single connected network (no partition), DAG forks are resolved by cumulative weight. The cumulative weight of a transaction is the sum of its own weight plus the weights of all transactions that directly or transitively reference it as a parent. When two transactions conflict (same sender, same nonce), the transaction with higher cumulative weight in the DAG is preferred. This mechanism provides probabilistic convergence similar to Bitcoin's longest-chain rule, but using stake-weighted attestation rather than proof-of-work.

**Cross-partition conflict resolution.** When partitions heal and the merge protocol runs (Section 9), conflict resolution uses a more sophisticated multi-factor algorithm that considers cumulative weight, visible stake percentage, and lexicographic hash tiebreaking. This is necessary because cumulative weight alone is not a fair comparison across partitions of different sizes - a transaction in a 60%-stake partition would naturally accumulate more weight than a transaction in a 40%-stake partition, even if both are equally valid.

---

## 8. Partition Tolerance {#8-partition-tolerance}

Consider a drone swarm operating in a contested RF environment where sub-groups regularly lose connectivity for 30-120 seconds. In this environment, partition mode is the expected operating state. The transaction classification system ensures mission-critical telemetry (Safe) continues uninterrupted, bounded resource allocation (BoundedSpend) proceeds within pre-configured limits, and consensus-critical operations (CpOnly) wait for full swarm reconnection.

### 8.1 Detection

Rinku implements a three-state partition detector that continuously monitors network health:

```
NORMAL  ──[visible stake < 2/3]──►  SUSPECTED  ──[timeout T_conf]──►  PARTITIONED
   ▲                                                                        │
   └──────────────[visible stake ≥ 2/3 for T_recovery]─────────────────────┘
```

| Parameter | Default | Description |
|-----------|---------|-------------|
| T_conf | 30s | Confirmation timeout before declaring partition |
| T_recovery | 10s | Quorum stability window before returning to normal |
| Stake visibility threshold | 66.66% | Minimum visible stake for normal operation |

The `PartitionDetector` runs every 5 seconds, computing the percentage of total stake reachable via healthy gossip peers. Two detection signals are used:

1. **Visible stake percentage.** The primary signal. The detector queries the gossip service for currently connected peers, maps them to their validator identities, and sums their stake. If the sum falls below 2/3 of total stake, the detector transitions to SUSPECTED.
2. **Checkpoint stall detection.** A secondary signal. If no new checkpoint has been finalized for 3x the expected checkpoint interval and the node has unfinalized transactions, this indicates a likely quorum failure. This signal catches cases where visible stake calculations are stale due to peer-list update delays.

The SUSPECTED state serves as a damping buffer - transient network hiccups (brief disconnections, route changes) do not trigger partition mode unless they persist for T_conf seconds. This prevents unnecessary mode switching in mildly unstable networks.

### 8.2 Partition Epochs

Upon entering PARTITIONED mode, a monotonically increasing `partition_epoch` is assigned. This epoch tags all transactions and provisional checkpoints created during the partition, enabling the merge protocol to distinguish pre-partition from during-partition state.

### 8.3 Provisional Checkpoints

During a partition, the checkpoint quorum threshold relaxes from 2/3 of total stake to **2/3 of visible stake** (minimum 1/3 of total stake as a safety floor). Checkpoints created under this relaxed quorum carry:

- `provisional: true`
- `partition_epoch: <epoch>`
- `visible_stake_pct: <fraction>`

Transactions finalized by provisional checkpoints receive **provisional finality** (see Section 2.3). They are treated as finalized for local operations but remain rollback-eligible during merge. Clients and applications are explicitly informed of the provisional status.

The safety floor of 1/3 total stake prevents a single isolated node from creating provisional checkpoints unilaterally - there must be a meaningful fraction of the validator set present in the partition for provisional operation to proceed.

### 8.4 Local Operation During Partition

All core operations continue during a partition, subject to the transaction classification system (Section 9.8):

- **Safe transactions** (DataOnly, Consolidation, Reward): Proceed without restriction. These are inherently merge-safe.
- **BoundedSpend transactions** (Transfer, Contract): Proceed up to the account's partition budget limit, if one is configured. Without a budget, they proceed without restriction but carry rollback risk.
- **CpOnly transactions** (Stake, Unstake, ClaimRewards): Rejected during partition. These operations modify the validator set or consensus-critical state and require full quorum for safety.

The partition is transparent to the application layer except for finality semantics (provisional vs. final) and CpOnly rejection. DAG tip selection, weight calculation, gas pricing, and smart contract execution operate identically in both modes.

### 8.5 Provisional Receipt Semantics

During partition mode, all receipts and VerifiableObjects carry the `partition_epoch` and `provisional: true` markers. A `ProofFreshness` attached to any VO generated during partition mode includes the partition epoch, enabling downstream consumers to make informed trust decisions. Applications that require settlement finality can choose to wait for post-merge reconciliation; applications that prioritize responsiveness (messaging, social features) can accept provisional receipts immediately.

---

## 9. Reconciliation Semantics & Transaction Taxonomy {#9-reconciliation}

This section describes the core protocol innovation: a deterministic 5-phase merge protocol that reconciles divergent partition state while preserving maximum valid work from all partitions. This section is intended to be the most rigorous in the paper and will be the primary target of formal analysis.

### 9.1 Determinism Requirements

All merge computation must produce identical results on every node given the same inputs. This is the foundational constraint - without it, nodes would diverge after merge, which is worse than the partition itself.

**Integer accounting:** All balance operations during merge use `u64` micro-units (1 RKU = 10^8 micro-RKU). Balances are converted from `f64` to micro-units at merge entry and converted back only after reconciliation is complete. This eliminates floating-point nondeterminism entirely within the merge path.

**Canonical ordering:** Transactions are totally ordered by:

1. Per-account nonce ascending
2. DAG depth from fork-point (topological distance)
3. Transaction hash lexicographic ascending (deterministic tiebreaker)

This is a strict total order - no two transactions can be "equal" because transaction hashes are unique. Every node computing this order on the same transaction set produces the identical sequence.

**Contract isolation:** Contract calls are **not re-executed** during merge. The merge algorithm operates only on the balance transfers recorded in the original transaction execution (amount + gas fee). This avoids nondeterminism from contract execution order, memory state, or environmental dependencies.

**Contract storage conflicts** are resolved separately via "last-write-wins by weight" on individual storage keys. For each key written by transactions in both partitions, the write from the transaction with higher cumulative weight wins.

**Known limitation: atomic multi-key updates.** The per-key "last-write-wins" rule may produce inconsistent contract state when contract logic depends on multiple storage keys being written atomically. For example, if a contract maintains an invariant `balance_a + balance_b == total` and Partition A updates `balance_a` while Partition B updates `balance_b`, the merged state may violate the invariant. Mitigation strategies include contract-level merge hooks (Section 15.7), storage namespacing by partition safety class, and restricting AP-mode contract calls to safe subsets via the transaction classification system.

### 9.2 Phase 1 - DAG Exchange

Both partitions exchange their `PartitionDAGDelta` containing all transactions, provisional checkpoints, and DAG edges created since the fork point (last common confirmed checkpoint). After exchange, both sides have the complete merged transaction set.

Exchange is performed via gossip protocol messages (`GossipMessage::MergePayload` and `GossipMessage::MergeResult`). The merge payload includes a `MergeRequest` containing:

- All transactions created during the partition epoch
- Account state snapshots from the partition
- Provisional checkpoint chain
- Fork point reference (last common confirmed checkpoint height and hash)

### 9.3 Phase 2 - Conflict Detection

The merged transaction set is scanned for two conflict types:

**Type 1 - Direct Double-Spend:** Same account + same nonce in both partitions. This requires the user to have deliberately crafted a conflicting transaction - it cannot happen accidentally because nonces are sequential. Unambiguous evidence of intentional double-spending.

**Type 2 - Economic Overdraft:** No nonce collision, but combined spending from both partitions exceeds the account's pre-partition balance. Example: Alice had 100 RKU at the fork point, spent 60 RKU (nonce 5) in Partition A and 60 RKU (nonce 5 in Partition B, which was locally valid because Partition B never saw nonce 5 from Partition A). Combined: 120 spent on 100 balance.

This may be innocent (the user didn't know about the partition and transacted on both sides) or opportunistic.

**Detection algorithm:**

1. Build a set of all `(account, nonce)` pairs across both partitions. Flag any duplicates as Type 1.
2. For each account that transacted in both partitions, compute pre-partition balance (from fork-point checkpoint), total sent and received in each partition. If combined net spend exceeds pre-partition balance, flag as Type 2.

### 9.4 Phase 3 - Weight Resolution

For each conflict, a deterministic winner is selected:

**Direct double-spends:** Winner is the transaction with higher cumulative DAG weight in the merged graph. If weights are within a 1.5x proximity threshold, tiebreaker is the partition with higher `visible_stake_pct` at the time of the provisional checkpoint that finalized the transaction. If still tied, the transaction with the lexicographically lower hash wins.

**Economic overdrafts:** All transactions from the conflicting account are ordered by nonce ascending, then cumulative weight descending. Starting from the pre-partition balance (in micro-units), transactions are replayed in this order. The first transaction that would cause an overdraft - and all subsequent transactions from that account - are marked as losers.

### 9.5 Phase 4 - Cascade Rollback

The key insight: rejecting a transaction doesn't just affect its sender - it affects everyone who received funds from that transaction and subsequently spent them. The cascade rollback algorithm traces these economic dependency chains.

**Algorithm (pseudocode):**

```
function cascade_rollback(losing_txs, all_txs):
    balances = snapshot_balances_at_fork_point()     // u64 micro-units
    surviving_txs = all_txs.exclude(losing_txs)
    ordered_txs = canonical_sort(surviving_txs)       // see Section 9.1
    rolled_back = Set()

    loop:
        newly_rolled_back = Set()
        balances = snapshot_balances_at_fork_point()  // reset each iteration
        expected_nonce = {}                            // per-account

        for tx in ordered_txs:
            if tx.hash in rolled_back: continue

            // Nonce continuity: if a prior nonce was rolled back,
            // all subsequent nonces from this account are invalid
            if tx.nonce > expected_nonce[tx.from]:
                newly_rolled_back.add(tx.hash)
                continue

            cost = to_micro(tx.amount) + to_micro(tx.gas)
            if balances[tx.from] < cost:
                newly_rolled_back.add(tx.hash)
                continue

            balances[tx.from] -= cost
            balances[tx.to] += to_micro(tx.amount)
            expected_nonce[tx.from] += 1

        if newly_rolled_back.is_empty(): break   // stable
        rolled_back.extend(newly_rolled_back)

    return (rolled_back, balances)
```

**Convergence guarantee:** Each iteration can only add rollbacks, never remove them. The valid transaction set shrinks monotonically. Since the transaction set is finite, the algorithm terminates.

**Proof of convergence.** Let S_i be the set of surviving transactions after iteration i. The algorithm guarantees S_{i+1} ⊆ S_i (monotonic shrinkage) because: (1) each iteration resets balances and replays from scratch, so previously-valid transactions remain valid if no new rollbacks affect their dependencies; (2) newly rolled-back transactions only occur when a dependency (balance or nonce) is broken by a prior rollback. Since S_i shrinks monotonically and is bounded below by the empty set, the algorithm converges in at most |S_0| iterations. In practice, cascades are shallow - most transactions don't depend on funds from rolled-back transactions - and convergence occurs in 1-3 iterations.

**Complexity:** Worst case O(n^2) where n is the number of transactions in the partition period. In practice, cascades are shallow - most transactions don't depend on funds from rolled-back transactions. A partition lasting hours with thousands of transactions is expected to reconcile in seconds.

### 9.6 Phase 5 - State Reconciliation

After cascade rollback completes:

1. **Account state rebuild:** `final_balances` from the cascade replay (in micro-units) are converted back to `f64` and written as canonical account state.
2. **DAG cleanup:** Direct conflict losers (nonce reuse - malicious) are removed from the DAG entirely. Cascade victims (innocent users whose upstream funds evaporated) are marked `rolled_back: true` and kept for auditability.
3. **Merge checkpoint:** A new checkpoint is created at `fork_point_height + 1` with:
   - `provisional: false`
   - `previous_hash` linking to the fork-point checkpoint
   - `finalized_tx_hashes` containing all surviving transactions from both partitions
   - `merge_report_hash` referencing the full MergeReport
   - Signed by the reunified validator quorum (must meet full 2/3 total stake threshold)
4. **Provisional chain retirement:** All provisional checkpoints from both partitions are archived to `merge_history` and removed from the active checkpoint chain.

### 9.7 Nonce Gap Behavior

When a transaction is rolled back, all subsequent transactions from that account (with higher nonces) are also rolled back, regardless of their individual validity. This is because Rinku enforces strict nonce sequentiality - a "gap" in the nonce sequence means the account's state is undefined for all operations after the gap.

This is a deliberate design choice: it prevents subtle state inconsistencies where an account's later transactions depend on side effects (balance changes, contract state mutations) of the rolled-back transaction, even if the later transactions appear independently valid.

**Why nonce-gap rollback is necessary.** Consider the alternative - nonce remapping, where surviving transactions are renumbered to fill gaps. This introduces several problems: (1) transaction hashes would change (the nonce is part of the signed data), invalidating signatures; (2) any VerifiableObject referencing the original transaction hash would become invalid; (3) the remapped transaction would need re-execution for contracts, violating the determinism requirement. Partial replay (skipping the gap and continuing with later nonces) is equally problematic because later transactions may depend on state changes from the skipped transaction - a transfer at nonce 3 may only be valid because nonce 2 received incoming funds.

Nonce-gap rollback is conservative - it potentially rolls back more transactions than strictly necessary. This is an intentional safety-over-liveness tradeoff: the cost of unnecessary rollbacks (user inconvenience) is far lower than the cost of state corruption from invalid partial replays.

**Important subtlety: cross-partition nonce filling.** When a transaction loses a direct conflict (e.g., local nonce 1 loses to remote nonce 1), the winning transaction fills the nonce slot. Subsequent nonces (2, 3, ...) from the same account are NOT rolled back - the sequence remains unbroken. Nonce-gap cascades only occur when no transaction (from either partition) fills a nonce slot.

### 9.8 Transaction Classification

Rinku implements a `PartitionSafety` classification system that governs which transactions may execute during partition mode. This is a protocol-level enforcement, not an application-layer convention - the gate is applied in the transaction acceptance path before DAG insertion.

| Classification | AP-Mode Behavior | Transaction Types | Rationale |
|---------------|-----------------|-------------------|-----------|
| **Safe** | Always allowed | DataOnly, Consolidation, Reward | Append-only or system-generated; no cross-account economic state mutation; inherently merge-safe |
| **BoundedSpend** | Allowed within partition budget | Transfer, Contract | May create cross-partition conflicts; bounded by optional per-account spending limit |
| **CpOnly** | Rejected during partition | Stake, Unstake, ClaimRewards | Modify validator set or consensus-critical state; require full quorum for safety |

**Partition budget system.** Accounts can optionally configure a `partition_budget` - a maximum amount spendable during any single partition epoch. When a BoundedSpend transaction is submitted during partition mode, the protocol checks:

1. Has the account configured a partition budget? If not, the transaction proceeds without restriction (but carries full rollback risk).
2. Would this transaction cause `partition_budget_spent + tx_amount` to exceed `partition_budget`? If so, the transaction is rejected.

Transactions within the partition budget are economically guaranteed to be merge-safe: even in the worst case (identical spending in both partitions), the combined spend cannot exceed the pre-partition balance if the budget is set to at most half the balance. This transforms partition tolerance from a probabilistic property to a deterministic one for opted-in accounts.

The budget is tracked via `partition_budget_spent` on the Account struct and is reset when the node enters a new partition epoch.

### 9.9 Three-Tier Receipt Model

The VerifiableObject system supports explicit receipt tiers as semantic markers:

- **TentativeReceipt:** Issued during partition mode. Carries provisional checkpoint anchor and partition epoch. Explicitly communicates "valid locally, subject to rollback." Applications can render these with appropriate UI affordances (e.g., a pending indicator).
- **FinalReceipt:** Issued after checkpoint finality during normal operation. Carries full BLS quorum signature. Irrevocable under honest majority assumption.
- **ReconciliationReceipt:** Issued after merge. Proves that a tentative transaction was either accepted (upgraded to final status in the merge checkpoint) or rejected (rolled back with reason code: `ConflictLoser`, `CascadeVictim`, `NonceContinuityGap`, `InsufficientBalanceAfterConflictResolution`).

These tiers are not separate data structures but semantic interpretations of the existing `VerifiableObject` with its `provisional`, `partition_epoch`, and `merge_report_hash` fields. Applications and wallets should inspect these fields to determine the receipt tier and display appropriate finality information to users.

---

## 10. Smart Contracts & WASM Runtime {#10-smart-contracts}

### 10.1 Runtime Architecture

Rinku executes smart contracts in a sandboxed WASM environment built on the `wasmi` interpreter:

- **Memory sandbox:** Configurable up to 256 pages (16 MB), with bounds-checked guest memory access
- **Import validation:** Only `rinku` and `env` namespaces are permitted; all other imports are rejected at deployment
- **Deterministic execution:** `wasmi` provides bit-identical execution across platforms

### 10.2 Dual Gas Metering

Gas is metered at two levels to prevent both instruction-level abuse and host-call abuse:

- **Fuel metering:** Every WASM instruction consumes interpreter fuel (default budget: 10,000,000 fuel units)
- **Host gas metering:** Expensive host operations charge additional gas through the `GasMeter`

Total gas = `(fuel_consumed / 100) + host_gas + base_gas + input_bytes_gas`

Where `input_bytes_gas = input_size * 16` (16 gas per byte of contract input).

**Host operation gas schedule:**

| Operation | Gas Cost | Rationale |
|-----------|----------|-----------|
| `base_execution` | 1,000 | Fixed cost per contract invocation |
| `storage_read` | 200 | Database lookup |
| `storage_write` | 5,000 | Database write + state hash update |
| `storage_delete` | 5,000 | Database delete + state hash update |
| `memory_alloc` | 3 | Per-page memory allocation |
| `transfer` | 8,000 | Balance mutation on two accounts |
| `mint` | 6,000 | Token minting operation |
| `burn` | 6,000 | Token burning operation |
| `emit` | 500 | Event serialization and broadcast |
| `hash` | 300 | Cryptographic hash (sha256/keccak256) |
| `balance_check` | 100 | Ledger state lookup (get_balance/get_staked) |
| `log` | 100 | Debug output (no state mutation) |

### 10.3 Host ABI

The `rinku` namespace exposes:

| Function | Description |
|----------|-------------|
| `storage_read`, `storage_write`, `storage_delete`, `storage_has` | Contract key-value storage (JSON-serialized values) |
| `get_caller`, `get_block_height`, `get_timestamp`, `get_input` | Execution context |
| `get_contract_id` | Self-referential contract address |
| `get_balance`, `get_staked` | Ledger queries |
| `transfer` | Native token transfers from contract balance |
| `emit_event` | Event emission for indexing and WebSocket subscribers |
| `emit_view_key` | Expose state fragments for stateless verification |
| `sha256`, `keccak256` | Cryptographic hashing |

### 10.4 Stateless dApp Standard (Proof-Carrying Contracts)

Contracts define `ViewKeySpec` schemas specifying which pieces of state should be exposed for external verification. Every mutating call returns a `StatefulReceipt` containing:

- View key values (specific state fragments selected by the contract)
- Merkle multiproof connecting those values to the checkpoint state root
- Finality certificate (checkpoint anchor with BLS signature)

This enables **persistently stateless clients** - applications that never store blockchain state locally but can verify any claim on demand using receipts. A mobile app can render a user's profile, balance, or social feed entirely from VerifiableObjects without maintaining a local database or trusting any server.

### 10.5 Contract SDK

Rinku provides a Rust SDK (`rinku-contract-sdk`) for contract development with the following macros and helpers:

- **`entrypoint!`** - Declares the contract's entry point function, handling input deserialization and output serialization.
- **`contract_init!`** - Declares the contract's initialization function, called once at deployment.
- **`contract_call!`** - Declares a callable contract function with automatic gas metering and error handling.

The SDK provides helper functions for common operations:

- **Storage:** `storage::get<T>()`, `storage::set<T>()`, `storage::delete()`, `storage::has()` - type-safe wrappers around the host ABI storage functions with automatic JSON serialization.
- **Transfers:** `token::transfer(to, amount)` - transfers RKU from the contract's balance.
- **Events:** `events::emit(name, data)` - emits a named event for indexing and WebSocket subscribers.
- **View keys:** `view::expose(key, value)` - registers a view key for inclusion in the `StatefulReceipt`.

Contracts are compiled to WASM using standard Rust toolchain targeting `wasm32-unknown-unknown`, then deployed via a `Contract` transaction with the base64-encoded WASM binary and initial state.

---

## 11. Economic Model {#11-economic-model}

### 11.1 Supply Schedule

| Parameter | Value |
|-----------|-------|
| **Maximum supply** | 30,000,000 RKU |
| **Genesis allocation** | 6,000,000 RKU (20%) |
| **Maximum emittable** | 24,000,000 RKU |
| **Halving interval** | 3,150,000 checkpoints (~1 year at 10s intervals) |
| **Total halvings** | 5 |
| **Minimum reward floor** | 0.122887 RKU per checkpoint |

**Emission schedule:**

| Halving | Checkpoints | Reward per Checkpoint | Cumulative Emitted |
|---------|------------|----------------------|-------------------|
| 0 | 0 – 3,149,999 | ~3.93 RKU | ~12,379,500 RKU |
| 1 | 3,150,000 – 6,299,999 | ~1.965 RKU | ~18,569,250 RKU |
| 2 | 6,300,000 – 9,449,999 | ~0.983 RKU | ~21,664,125 RKU |
| 3 | 9,450,000 – 12,599,999 | ~0.491 RKU | ~23,211,563 RKU |
| 4 | 12,600,000 – 15,749,999 | ~0.246 RKU | ~23,985,281 RKU |
| 5+ | 15,750,000+ | 0.122887 RKU (floor) | Approaches 30,000,000 |

The emission curve follows a geometric decay with a floor, ensuring that block rewards never reach zero - validators always have a base-layer incentive to participate, even after the majority of tokens have been emitted. The hard cap of 30M RKU is asymptotically approached but never exceeded due to the floor mechanism and total supply enforcement in the emission logic.

### 11.2 Checkpoint Rewards

Checkpoint rewards are distributed when a checkpoint is finalized. The reward amount is determined by the emission schedule (Section 11.1) and is distributed to all active validators proportional to their effective weight:

**Distribution formula:**

```
validator_reward = checkpoint_reward * (validator_effective_weight / total_effective_weight)
```

Where `effective_weight` is computed using the dual-weight system (Section 11.3). The total checkpoint reward is minted as new supply, subject to the hard cap - if minting the full reward would exceed 30,000,000 RKU, the reward is reduced to the remaining mintable amount.

Rewards are credited directly to the validator's liquid balance (not to their staked balance), allowing validators to compound their stake through explicit re-staking or use rewards for other purposes.

### 11.3 Weighted Proof-of-Stake (WPoS)

Checkpoint rewards are distributed through a dual-weight system:

- **Stake weight (70%):** Proportional to amount staked
- **Age weight (30%):** Rewards long-term active participation; requires minimum 100 RKU bond; decays 10% per missed checkpoint

### 11.4 Staking Requirements

| Parameter | Value |
|-----------|-------|
| **Minimum stake** | 100 RKU |
| **Minimum stake age for rewards** | 15 seconds |
| **Unstake cooldown** | 24 hours |
| **Slashing unbonding period** | 14 days |

### 11.5 Additional Reward Streams

- **Tip rewards (1%):** Distributed as incentives for specific network actions
- **Witness rewards (0.2%):** Incentivize DAG connectivity by rewarding nodes that reference other transactions as parents

### 11.6 Gas Fee Model

Rinku implements an EIP-1559-inspired dynamic gas pricing mechanism:

| Parameter | Value |
|-----------|-------|
| **Target throughput** | 150 transactions per 15-second period |
| **Adjustment factor** | 12.5% per period |
| **Minimum gas price** | 0.001 RKU |
| **Maximum gas price** | 10.0 RKU |

The gas price adjusts each period: if the actual transaction count exceeds the target, the price increases by the adjustment factor; if below target, it decreases. This creates a self-regulating fee market that responds to demand without requiring explicit fee auctions.

### 11.7 Adaptive Fee Burn

Transaction fees are split between burning and validator rewards:

- **Burn ceiling:** 30% of fees
- **Burn scaling:** Burn percentage increases linearly as circulating supply approaches 50% of the hard cap. At 0% of max supply, no fees are burned; at 50% (15M RKU), the full 30% ceiling is reached.
- **Validator floor:** Validators always receive at least 70% of fees

The adaptive burn creates deflationary pressure that increases as the token supply grows, counterbalancing emission inflation and creating a natural equilibrium. At maturity (when emission is at the floor rate), the burn mechanism may offset or exceed emission, potentially making the token supply effectively stable or mildly deflationary depending on transaction volume.

### 11.8 Micro-Unit Precision

All internal accounting uses `u64` micro-units with 8 decimal places (1 RKU = 100,000,000 micro-RKU). This eliminates floating-point precision errors, which is particularly critical for deterministic merge reconciliation (Section 9).

---

## 12. Slashing & Economic Security {#12-slashing}

### 12.1 Violation Types

| Violation | Severity | Penalty |
|-----------|----------|---------|
| **Nonce Reuse (Cross-Partition Double-Spend)** | Malicious | 10% balance penalty + 100% stake slash + 0.50 permanent reputation penalty |
| **Cross-Partition Economic Overdraft** | Gray area | 0.10 reputation penalty with linear decay over 100 checkpoints |
| **Cascade Victim** | Innocent | No penalty |
| **Double-Sign (same height, different hash)** | Malicious | 15% stake slash |
| **Invalid Checkpoint Proposal** | Malicious | 25% stake slash |
| **Receipt/Proof Tampering** | Malicious | 25% stake slash |
| **Invalid Proof/Witness Submission** | Malicious | 15-20% stake slash |
| **Liveness Failure (3+ missed checkpoints)** | Negligent | 5% stake slash (increasing to 10% for repeat offenses) |

The graduated penalty structure reflects a key design principle: **the protocol distinguishes intent.** Nonce reuse requires deliberate action and is treated as malicious. Economic overdraft may be accidental and receives a soft, recoverable penalty. Cascade victims are blameless and bear no cost.

### 12.2 Reputation & Weight Modifier

Accounts with reputation penalties receive reduced weight in all weight calculations:

```
effective_weight = base_weight * (1.0 - reputation_penalty)
```

This reduces the influence of penalized accounts in DAG weight calculations, tip selection, and consensus voting without requiring immediate ejection from the network.

### 12.3 Penalty Decay

Cross-partition overdraft reputation penalties decay linearly over 100 checkpoints (~16 minutes at 10s intervals), allowing honest users caught in an ambiguous situation to recover their standing. Nonce-reuse penalties are permanent and do not decay.

The decay is computed as:

```
remaining_penalty = original_penalty * max(0, 1.0 - (current_height - penalty_height) / 100)
```

Where `penalty_height` is the checkpoint at which the penalty was applied (stored in `penalty_decay_checkpoint` on the Account struct).

### 12.4 Game-Theoretic Analysis

**Nonce-reuse attack cost.** An attacker attempting a cross-partition double-spend faces: 10% balance confiscation + 100% stake slash + 0.50 permanent reputation penalty. For this to be profitable, the double-spent amount must exceed `0.10 * balance + 1.00 * stake + NPV(weight_reduction)`. The permanent reputation penalty means the attacker's future transaction weight is halved indefinitely, reducing their influence in all protocol interactions. For any account with meaningful stake, the expected cost far exceeds the maximum double-spend gain (which is bounded by the account's pre-partition balance).

**Economic overdraft opportunism.** An attacker who intentionally spends in both partitions faces a 0.10 reputation penalty with 100-checkpoint decay. The cost is temporary weight reduction. This penalty is intentionally mild because the attack surface is limited (the attacker can only spend their own balance) and the behavior may be innocent. The partition budget system provides a stronger mitigation for users who want guaranteed merge safety.

**Cascade attack analysis.** An attacker cannot deliberately cause cascades against specific victims without first losing their own funds in a direct conflict. Cascade rollbacks are a second-order effect - they require the attacker to sacrifice their own transaction first. The attacker bears the full penalty for the initiating conflict; cascade victims bear no penalty. The attacker cannot profit from cascading because the rolled-back funds return to their pre-partition state, not to the attacker.

---

## 13. Privacy Layer {#13-privacy}

### 13.1 ZK-SNARK Integration

Rinku supports optional privacy-preserving transactions using Groth16 ZK-SNARKs:

- Privacy proofs generated client-side using `snarkjs` / `circomlibjs`
- Verification artifacts hosted on CDN for client-side proof generation
- Poseidon-based Merkle tree for efficient in-circuit state verification

### 13.2 Selective Disclosure

The combination of Sparse Merkle Tries and self-contained proofs enables selective disclosure: users can prove specific financial or state facts (e.g., "my balance exceeds X" or "I am a member of set Y") without revealing their full transaction history or exact balance.

### 13.3 Contract-Layer Privacy

Privacy features at the contract layer complement the optional ZK privacy layer:

- **Sender obfuscation.** Contracts can implement a mixer pattern where transfers are routed through the contract, breaking the direct link between sender and recipient on the public DAG. The contract maintains internal transfer records encrypted to the participants.
- **Gasless meta-transactions.** A relayer pattern allows users to submit transactions through a third party who pays the gas fee. The user signs the transaction payload; the relayer wraps it in an outer transaction and submits it to the network. The contract verifies the inner signature and executes on behalf of the original user. This provides economic privacy (the user's account doesn't need to hold RKU for gas) and can be combined with sender obfuscation for stronger anonymity.

---

## 14. Networking & P2P Protocol {#14-networking}

### 14.1 Gossipsub

Rinku's gossip protocol operates on the `rinku/1.0.0` topic with the following message types:

| Message Type | Priority | Description |
|-------------|----------|-------------|
| `Transaction` | Normal | Signed transaction propagation |
| `TipAnnouncement` | Normal | Current DAG tips and size (triggers sync if peer is ahead) |
| `CheckpointAnnouncement` | High | New finalized checkpoint with transaction bodies |
| `CheckpointSignature` | High | Individual validator signature for checkpoint voting |
| `FastPathBroadcast` | High | Mysticeti-style fast-path broadcast for immediate acceptance |
| `FastPathAck` | High | Validator acknowledgment for fast-path transaction |
| `BloomAnnouncement` | Low | Bandwidth-efficient advertisement of known transactions |
| `PeerDiscovery` | Low | Shares known peer addresses for mesh expansion |
| `ConflictResolution` | Normal | Broadcasts conflict resolution decisions |
| `SlashingEvidence` | High | Proof of validator misbehavior (double-signing) |
| `WeightVote` | Normal | Validator vote for transaction trust weighting |
| `MergePayload` | High | Partition merge request with DAG delta |
| `MergeResult` | High | Partition merge response with reconciliation data |
| `SyncRequest` | Normal | Request for missing transactions |
| `SyncResponse` | Normal | Response with requested transactions |

**Bloom filter optimization.** To reduce bandwidth, nodes periodically broadcast `BloomAnnouncement` messages containing a Bloom filter of their known transaction hashes. The filter uses double SHA-256 hashing for optimal bit distribution. Before sending a transaction to a peer, the node checks the peer's Bloom filter - if the transaction is likely already known, it is not retransmitted. The filter includes the node's checkpoint height and tip count, enabling peers to detect if they are behind and should initiate sync.

**Propagation batching.** To prevent message storms under high transaction load, the gossip service uses a background propagation task with a `MAX_PROPAGATION_BATCH` of 100 transactions. Pending transactions are accumulated and broadcast in batches, amortizing the per-message overhead of gossipsub.

**Deduplication.** A `BoundedHashSet` (`known_txs`) tracks recently seen transaction hashes to prevent infinite gossip loops. The set has a bounded capacity and evicts oldest entries when full.

### 14.2 Lock-Free Message Handling

Rinku's P2P receive path is designed to eliminate mutex contention on the critical message-processing path:

**Channel-based architecture.** The `NetworkHandle` wraps the libp2p swarm and exposes message channels. During initialization, the gossip message receiver (`message_rx`) and sync request receiver (`sync_incoming_rx`) are extracted from the `NetworkHandle` **before** it is wrapped in `Arc<Mutex<>>`. These channels are passed directly to handler tasks via `set_p2p_channels()`.

**Implications.** The message receive path never acquires the `NetworkHandle` mutex. Incoming gossip messages flow from the libp2p event loop → mpsc channel → `run_p2p_receiver` task → `handle_message` processing, entirely lock-free. Similarly, sync requests flow through a separate channel to `run_sync_request_handler`. Sync response sending uses a cloned `command_tx` sender, which is also lock-free (mpsc senders are cheaply cloneable).

This architecture eliminates the 5-25ms polling latency per message that would occur if the receive path needed to acquire a mutex on every incoming message, which is critical for maintaining sub-second fast-path acceptance latency under load.

### 14.3 Connection Management

**Idle timeout.** Connections have a 600-second idle timeout, configured to prevent premature `KeepAliveTimeout` disconnects on low-traffic deployments where minutes may pass between gossip messages.

**Mesh maintenance.** The network service periodically checks if the number of validated peers is below `MIN_MESH_PEERS` (1). If the mesh is unhealthy, it re-dials bootstrap peers to restore connectivity. The `InsufficientPeers` publish error (which occurs during startup or reconnection when no gossipsub peers are available) is logged at trace level to avoid log noise during expected transient states.

**Peer discovery.** Nodes exchange `PeerDiscovery` messages containing their known peer addresses. New peers are added to the connection pool and validated against the validator identity service. The `/api/peers` endpoint exposes the current P2P peer list as the primary field, with legacy HTTP peer information included only when non-empty.

**Validator identity verification.** The `ValidatorIdentityService` maps P2P peer IDs to validator addresses and BLS public keys. When a new peer connects, the service verifies the peer's claimed identity against the registered validator set. This prevents Sybil attacks at the network layer - only peers corresponding to staked validators contribute to stake visibility calculations for partition detection.

---

## 15. Future Work {#15-future-work}

### 15.1 CRDT-Compatible State Types

For contract storage, introduce merge-friendly data types (sets, append-only logs, max counters, OR-maps) that can be safely updated during partitions without conflict. Ideal for social, messaging, and collaborative applications. Would integrate with the transaction classification system (Section 9.8) to automatically determine AP-safety - contracts that exclusively use CRDT-compatible state types could be classified as Safe rather than BoundedSpend.

### 15.2 Object Ownership Model

Explore single-writer object ownership where owned objects can be processed during partitions without conflict, while shared objects remain CP-only. This model is inspired by Sui's object-centric programming model: if an object has a single owner, mutations by that owner are inherently conflict-free across partitions. The challenge is integrating this with Rinku's account-based (rather than object-based) state model.

### 15.3 Cross-Chain Proof Composability

Leverage BYOP and VerifiableObjects for cross-chain interoperability. Since VOs are self-contained and carry their own verification data, a Rinku VO could be submitted to a contract on another chain (or vice versa) as a `ProofInput`. The receiving chain would verify the proof's BLS signature against a registered Rinku validator set root. This pattern enables receipt-composable bridges without trusted relayers - the bridge contract verifies the proof's mathematical validity rather than trusting a third party to relay state.

### 15.4 Contract-Level Merge Hooks

Allow contracts to define custom merge resolution logic for their storage, replacing the default "last-write-wins by weight" rule with application-aware conflict resolution. This would address the pathological contract state corruption risk identified in Section 9.1. A contract could implement a `merge_resolve(key, local_value, remote_value, local_weight, remote_weight) -> value` function that is invoked during Phase 5 of the merge protocol for each conflicting storage key.

---

## 16. Conclusion {#16-conclusion}

Rinku occupies a distinct position in the distributed ledger design space. Rather than making a fixed CAP tradeoff, it dynamically navigates the consistency-availability spectrum based on network conditions: strong checkpoint finality when the network is connected, provisional availability during partitions, and deterministic convergence when partitions heal.

The core contribution is not the mode-switching mechanism itself - dynamically adjusting consistency levels is a well-studied concept in distributed systems. The contribution is the set of protocol mechanisms that make this approach practical for a financial ledger:

1. **The transaction classification system** (Section 9.8) transforms partition tolerance from a binary property (available or not) into a graduated spectrum. Safe operations continue without restriction; bounded-spend operations proceed within configurable risk limits; consensus-critical operations halt until quorum is restored. This gives applications and users explicit control over their partition-mode risk exposure.
2. **The 5-phase merge protocol** (Section 9) provides deterministic reconciliation with formally provable convergence. Integer micro-unit accounting eliminates floating-point nondeterminism. Canonical transaction ordering ensures all nodes compute identical results. The cascade rollback algorithm traces economic dependencies exhaustively, preventing subtle state corruption from partial replays.
3. **Graduated economic deterrence** (Section 12) distinguishes intent. Deliberate double-spending (nonce reuse) is expensive and permanent. Accidental overdrafts receive soft, recoverable penalties. Cascade victims bear no cost. This penalty structure makes rational exploitation unprofitable while avoiding punishing honest users caught in ambiguous situations.
4. **VerifiableObjects** (Section 3) collapse the infrastructure requirements for trust. A `rinku://vo/` URL carries everything needed to verify a claim - no full node, no RPC endpoint, no ongoing network connection. This makes Rinku's proofs inherently portable, shareable, and composable - they can be passed as transaction parameters (BYOP), embedded in QR codes, or verified entirely offline.
5. **Proof-carrying contracts** (Section 10.4) extend this portability to smart contract state. Contracts define which state fragments should be provable; every execution produces a `StatefulReceipt` with Merkle proofs and finality certificates. Applications can be persistently stateless - rendering verified data from receipts rather than maintaining local state.

Together, these mechanisms create a distributed ledger designed for environments where network partitions are a routine operating condition rather than an exceptional failure. Rinku does not claim to solve the fundamental impossibility results of distributed systems - it claims to make a practically useful navigation of those constraints for the specific domain of decentralized financial infrastructure in mesh-native environments.

---

## Appendices

### A. Formal Definitions

**Definition 1 (Safety).** The Rinku protocol satisfies safety if no two finalized (non-provisional) checkpoints at the same height contain conflicting state roots. Under the honest majority assumption (>2/3 stake honest), safety holds because producing conflicting finalized checkpoints requires >1/3 stake to equivocate (sign two different hashes at the same height), which is detected and slashed.

**Definition 2 (Liveness - Normal Mode).** The protocol satisfies liveness in normal mode if every submitted valid transaction is eventually included in a finalized checkpoint, assuming >2/3 stake is reachable and the leader election mechanism produces a live leader within bounded time. The leader fallback mechanism (Section 6.4) ensures liveness even if the primary leader is offline.

**Definition 3 (Liveness - Partition Mode).** The protocol satisfies partition liveness if every submitted valid transaction classified as Safe or BoundedSpend (within budget) is included in a provisional checkpoint within bounded time, assuming the local partition contains ≥1/3 total stake. CpOnly transactions do not satisfy liveness during partition by design.

**Definition 4 (Convergence).** The merge protocol satisfies convergence if, given the same set of transactions from both partitions and the same fork-point state, every node independently computes identical post-merge state. This follows from: (a) integer micro-unit accounting eliminating floating-point nondeterminism, (b) canonical ordering producing a strict total order, and (c) the cascade rollback algorithm being a deterministic function of its inputs with provable termination.

**Definition 5 (Cascade Termination).** Let S_0 be the initial set of surviving transactions (after conflict resolution). The cascade rollback algorithm produces a sequence S_0 ⊇ S_1 ⊇ S_2 ⊇ ... that converges in at most |S_0| iterations. Proof: each iteration either removes at least one transaction (|S_{i+1}| < |S_i|) or produces no new rollbacks (S_{i+1} = S_i, and the algorithm terminates). Since |S_i| ≥ 0 and decreases by at least 1 per non-terminal iteration, the algorithm terminates in at most |S_0| steps.

### B. Parameter Reference

| Parameter | Value | Section | Description |
|-----------|-------|---------|-------------|
| MAX_SUPPLY | 30,000,000 RKU | 11.1 | Hard token supply cap |
| GENESIS_ALLOCATION | 6,000,000 RKU | 11.1 | Initial token distribution |
| HALVING_INTERVAL | 3,150,000 checkpoints | 11.1 | Reward halving period |
| MIN_REWARD_FLOOR | 0.122887 RKU | 11.1 | Minimum checkpoint reward |
| CHECKPOINT_INTERVAL | ~10 seconds | 6.2 | Target time between checkpoints |
| FAST_PATH_QUORUM | 66.7% active stake | 6.1 | Fast-path acceptance threshold |
| CHECKPOINT_QUORUM | 66.66% total stake | 6.2 | Checkpoint finality threshold |
| SUPER_MAJORITY | 75% total stake | 6.3 | Higher-security operations |
| LEADER_TIMEOUT | 45 seconds | 6.4 | Fallback leader election trigger |
| PROPAGATION_GRACE | 5,000 ms | 6.2 | Transaction propagation window |
| MIN_STAKE | 100 RKU | 11.4 | Minimum validator stake |
| UNSTAKE_COOLDOWN | 24 hours | 11.4 | Stake withdrawal delay |
| UNBONDING_PERIOD | 14 days | 11.4 | Slashing vulnerability window |
| MIN_GAS_PRICE | 0.001 RKU | 11.6 | Gas price floor |
| MAX_GAS_PRICE | 10.0 RKU | 11.6 | Gas price ceiling |
| GAS_TARGET | 150 tx / 15s | 11.6 | Target transaction throughput |
| GAS_ADJUSTMENT | 12.5% | 11.6 | Per-period price adjustment |
| BURN_CEILING | 30% | 11.7 | Maximum fee burn percentage |
| MAX_SAMPLED_TIPS | 16 | 7.2 | Maximum parent references per tx |
| PARTITION_THRESHOLD | 66.66% visible stake | 8.1 | Partition detection threshold |
| T_CONF | 30 seconds | 8.1 | Partition confirmation timeout |
| T_RECOVERY | 10 seconds | 8.1 | Quorum recovery window |
| PROVISIONAL_FLOOR | 33.33% total stake | 8.3 | Minimum stake for provisional checkpoints |
| WEIGHT_PROXIMITY | 1.5x | 9.4 | Weight tiebreaker proximity threshold |
| NONCE_REUSE_BALANCE_PENALTY | 10% | 12.1 | Balance confiscation for double-spend |
| NONCE_REUSE_STAKE_SLASH | 100% | 12.1 | Stake slash for double-spend |
| NONCE_REUSE_REPUTATION | 0.50 permanent | 12.1 | Permanent reputation penalty |
| OVERDRAFT_REPUTATION | 0.10 decaying | 12.1 | Recoverable reputation penalty |
| REPUTATION_DECAY_PERIOD | 100 checkpoints | 12.3 | Linear decay window (~16 min) |
| DOUBLE_SIGN_SLASH | 15% | 12.1 | Stake slash for checkpoint equivocation |
| IDLE_TIMEOUT | 600 seconds | 14.3 | P2P connection idle timeout |
| MIN_MESH_PEERS | 1 | 14.3 | Minimum gossipsub mesh size |
| MAX_PROPAGATION_BATCH | 100 | 14.1 | Transaction propagation batch size |
| WASM_MAX_PAGES | 256 (16 MB) | 10.1 | Contract memory limit |
| WASM_DEFAULT_FUEL | 10,000,000 | 10.2 | Default instruction fuel budget |
| MICRO_UNIT_SCALE | 10^8 | 11.8 | Micro-units per RKU |

### C. Benchmarks

WIP

#### C.1 Throughput

TODO

#### C.2 Acceptance Latency (Fast-Path Confirmed Only)

| Percentile | Latency |
|------------|---------|
| p50 | 43–44 ms |
| p95 | ~200 ms |
| p99 | ~500 ms |
| Min | ~22 ms |

Measured as time from submission to fast-path confirmation status. Only confirmed samples are included; transactions that did not achieve fast-path confirmation within 10s are excluded (they proceed to checkpoint finality instead). The p50 of ~43ms demonstrates sub-second acceptance for the majority of transactions via Mysticeti-FPC fast-path consensus.

#### C.3 Finality Latency (Checkpoint Inclusion)

| Percentile | Latency |
|------------|---------|
| p50 | ~10,300 ms |
| p95 | ~10,400 ms |
| p99 | ~15,400 ms |

Finality latency is dominated by the checkpoint interval (10s). Transactions that do not finalize within 60s are capped and included in the distribution. The tight p50–p95 band (~10.3s) aligning with the checkpoint interval confirms correct Mysticeti-FPC operation. The p99 tail reflects occasional transactions that straddle checkpoint boundaries.

#### C.4 Proof Generation & Size

| Proof Type | Generation Time | Response Size | Success Rate |
|------------|----------------|---------------|--------------|
| Account proof (Merkle inclusion) | 21 ms | 1,953 B | 100% (5/5) |
| Transaction proof | 26 ms | 1,841 B | 100% (5/5) |
| Self-contained proof (VO URL) | 21 ms | 1,703 B | 100% (5/5) |
| Batch proof (multi-receipt) | 22 ms | TODO | TODO |

All proof types are generated in under 30ms. Self-contained VerifiableObject URLs encode at ~1.7KB, enabling URL-portable verification without external state. Account proofs include the Sparse Merkle Trie inclusion path and weigh ~2KB.

---

## References {#references}

[1] Tran, J.A., Ramachandran, G.S., Shah, P.M., Danilov, C., Santiago, R.A., Krishnamachari, B. "SwarmDAG: A Partition Tolerant Distributed Ledger Protocol for Swarm Robotics." *Ledger*, Vol. 4, Supplement 1, pp. 25–31, 2019. DOI: [10.5195/ledger.2019.174](https://doi.org/10.5195/ledger.2019.174)

[2] Raikwar, M., Polyanskii, N., Müller, S. "SoK: DAG-based Consensus Protocols." arXiv:2411.10026, 2024. [https://arxiv.org/abs/2411.10026](https://arxiv.org/abs/2411.10026)

[3] Babel, K., Chursin, A., Danezis, G., Kichidis, A., Kokoris-Kogias, L., Koshy, A., Sonnino, A., Tian, M. "Mysticeti: Reaching the Limits of Latency with Uncertified DAGs." arXiv:2310.14821, 2023. [https://arxiv.org/abs/2310.14821](https://arxiv.org/abs/2310.14821)

[4] Nakamoto, S. "Bitcoin: A Peer-to-Peer Electronic Cash System." 2008. [https://bitcoin.org/bitcoin.pdf](https://bitcoin.org/bitcoin.pdf)

[5] Boneh, D., Drijvers, M., Neven, G. "BLS Multi-Signatures With Public-Key Aggregation." 2018. [https://crypto.stanford.edu/~dabo/pubs/papers/BLSmultisig.html](https://crypto.stanford.edu/~dabo/pubs/papers/BLSmultisig.html)
