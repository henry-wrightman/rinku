# Rinku: A URL-Native Distributed Ledger

**Abstract.** A distributed ledger where URLs serve as the canonical portable representation of transactions, proofs, and state transitions. Self-contained proofs embedded in URLs enable trustless verification without trusted infrastructure—data availability is provided by the sender or network. We propose a DAG-based consensus mechanism with weight-based Sybil resistance combining stake and account age. The system achieves per-transaction finality through periodic checkpoints, implements deflationary tokenomics with halving-based emission, and supports extensible smart contracts. The result is a ledger where the link itself is the proof.

## 1. Introduction

Traditional blockchains require nodes to verify state. Users must trust infrastructure providers or run their own nodes. We eliminate the need for trusted infrastructure by encoding the complete verification path in URLs—data availability is provided by the sender or network, but trust is not required.

The problem with existing systems:
1. **Infrastructure dependency** - Verification requires trusted nodes
2. **State opacity** - Users cannot independently verify without syncing
3. **Proof complexity** - Light client proofs require specialized tooling

Rinku solves this by making URLs proof-carrying. A transaction URL contains its ancestry back to a finalized checkpoint anchor, transaction signatures, and sufficient data for verification. Through DEFLATE compression, proof receipts fit within QR code limits for simple transactions, with full finality certificates available as shareable URLs for high-assurance use cases.

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
- The transaction and its hash
- Ancestry chain to last checkpoint (recursive parent bundles)
- Checkpoint anchor (id, merkleRoot, height, signatureCount)

### 2.3 Proof Size Analysis

Empirical measurement using high-entropy data (random hex hashes, random addresses, real ECDSA signature lengths):

| Proof Type | Transactions | JSON Size | URL Length |
|------------|--------------|-----------|------------|
| Single tx | 1 | 596 bytes | 596 chars |
| 1-depth ancestry | 2 | 978 bytes | 773 chars |
| 2-depth ancestry | 3 | 1,359 bytes | 941 chars |
| 5-depth ancestry | 6 | 2,501 bytes | 1,411 chars |
| 10-depth ancestry | 11 | 4,407 bytes | 2,228 chars |
| DAG 3-depth (15 txs) | 15 | 7,450 bytes | 3,709 chars |
| DAG 4-depth (31 txs) | 31 | 15,304 bytes | 7,248 chars |

**What is included:**
- Full transaction data (from, to, amount, fee, nonce, ts, sig)
- Transaction hash (64 chars)
- Checkpoint anchor (checkpointId, merkleRoot, height, signatureCount)
- Recursive parent bundles

**What is NOT included (to minimize size):**
- Full validator signatures array (only signatureCount for attestation)
- Full txHashes array from checkpoint
- Merkle proof paths (add ~100-500 bytes if needed)

DEFLATE compression achieves ~40-55% reduction on transaction JSON. Compression gains come primarily from repeated field names and structural redundancy (keys like "from", "to", "amount" appear in every transaction), even when values are high-entropy hex strings.

### 2.4 Platform Compatibility

QR codes require byte mode for base64url encoding (contains `-`, `_`, `/`):

| Platform | Limit | Single tx | 5-depth | 15-tx DAG |
|----------|-------|-----------|---------|-----------|
| QR Code L (7% EC) | 2,953 bytes | ✓ | ✓ | ✗ |
| QR Code M (15% EC) | 2,331 bytes | ✓ | ✓ | ✗ |
| QR Code Q (25% EC) | 1,663 bytes | ✓ | ✓ | ✗ |
| QR Code H (30% EC) | 1,273 bytes | ✓ | ✗ | ✗ |
| Firefox | 65,536 chars | ✓ | ✓ | ✓ |
| Chrome/Edge | 2 MB | ✓ | ✓ | ✓ |
| nginx/Apache (common config) | 8,192 chars | ✓ | ✓ | ✓ |

**Practical guidance:**
- Single transactions and short ancestry chains (1-5 depth) fit in QR codes
- Complex DAG proofs (15+ txs) require URL sharing via links, not QR
- All proofs fit comfortably in browser URL limits

### 2.5 Proof Profiles

Rinku supports three verification profiles with different security/size tradeoffs:

**Profile A: Receipt (QR-compatible)**
- Size: 600-2,300 chars (fits QR for ≤5 depth)
- Includes: tx data, signature, hash, ancestry chain, checkpoint anchor (id, merkleRoot, height, signatureCount)
- Guarantees: Transaction integrity, signature validity, ancestry consistency
- Trust assumption: Verifier trusts the checkpoint anchor was validly signed by ≥2/3 stake
- Use case: Point-of-sale receipts, payment confirmations, audit trails

**Profile B: Full Finality Certificate**
- Size: 3,000-10,000+ chars (URL sharing only)
- Adds: Merkle inclusion path (txHash → merkleRoot), aggregated validator signatures or signature bitmap, validator set commitment
- Guarantees: Cryptographic proof of checkpoint inclusion and validator quorum
- Trust assumption: Verifier knows genesis validator set or a pinned checkpoint
- Use case: High-value settlements, cross-chain bridges, legal evidence

**Profile C: Self-Contained Proof (v5 - MerkleSumTree Multi-Proof)**
- Size: 1,000-2,500 chars typical; QR-compatible for committees N ≤ 21, URL-shareable for larger
- URL format: `rinku://sp/{base64url-deflate-packed}` (canonical scheme; HTTP URLs are transport wrappers)
- Includes: chainId (network identifier), tx data, full checkpoint header, Merkle inclusion proof, BLS aggregated signature (48 bytes), MerkleSumTree multi-proof (signer leaves + shared auxiliary nodes), validatorSumTreeRoot (hash + totalWeight)
- Guarantees: **Fully offline verification** - verifier derives totalWeight from MerkleSumTree root
- Trust assumption: **Chain identity only** - verifier must know expected chainId; totalWeight is cryptographically bound to validator set via MerkleSumTree; BLS public keys are PoP-verified at registration (see E.2.1)
- Security: validatorSumTreeRoot (containing both hash and totalWeight) is included in BLS signing hash, preventing both signer weight forgery AND denominator attacks
- Use case: Offline verification, air-gapped systems, cross-chain bridges, legal evidence
- **Size scaling**: With multi-proof (v5 default), proof size is `~O(N)` where `N` = committee size. Without multi-proof (v4), size grows as `O(k · log₂N)` where `k` = signers
  - **Multi-proof optimization** (default in v5): Share sibling nodes across signers, reducing to `~N` nodes total (60-75% smaller than per-signer proofs). See Appendix F.3.1 and F.5
  - **QR-compatible** (≤2,953 bytes): Requires packed binary + multi-proof AND committee N ≤ 21. JSON encoding will NOT fit QR
  - **URL-shareable** (always): All proofs fit browser URL limits (65KB+) with any reasonable committee size
  - **Packed encoding required for QR**: Parallel byte arrays for siblings (32B hash + 8B weight each), varint indices, raw bytes for pubkeys. See Appendix F
  - **Recommendation**: Default to URL sharing; QR codes viable for committees N ≤ 21 with packed + multi-proof

Most use cases are served by Profile A receipts. Profile B provides full finality when validator set is known. **Profile C is the gold standard** - completely self-contained proofs with cryptographically-bound totalWeight, enabling true offline verification without any trust assumptions beyond the cryptographic primitives.

### 2.6 Verification Process

Verification differs by profile:

**Profile A (Receipt) Verification:**
1. Decode and decompress the payload
2. Verify the transaction signature against sender's public key
3. Validate hash integrity (recompute and compare)
4. Trace ancestry to checkpoint anchor
5. Trust checkpoint attestation via signatureCount (requires trust in checkpoint source)

**Profile B (Full Finality) Verification:**
1. All Profile A steps (1-4)
2. Verify Merkle inclusion path (txHash → txMerkleRoot)
3. Verify aggregated validator signatures against known validator set
4. Confirm signer weight ≥ 67% of total stake
5. *Requires*: Trust anchor (genesis validator set or pinned checkpoint)

**Profile C (Self-Contained) Verification:**
1. All Profile A steps (1-4)
2. Verify Merkle inclusion path (txHash → txMerkleRoot)
3. Extract signer indices from bitmap
4. Verify MerkleSumTree multi-proof: place leaves + auxiliary nodes, reconstruct root level-by-level
5. Verify reconstructed root matches claimed validatorSumTreeRoot (hash AND totalWeight)
6. Extract signer BLS public keys from multi-proof leaves
7. Compute aggregate public key: `pk_agg = Σ pk_i` (G2 point addition)
8. Verify BLS aggregated signature: `BLS.verify(pk_agg, signingHash, σ_agg)`
9. Sum signer weights from multi-proof leaves; confirm ≥ 67% of derived totalWeight
10. Verify chainId in signing hash matches expected network
11. *Requires*: Chain identity (chainId) + cryptographic primitives; BLS keys must be PoP-verified at registration (see E.2.1)

The URL carries the proof. Profile A requires trust in the checkpoint source. Profile B requires a known validator set. Profile C is self-contained for *finality verification*—but chain identity binding is required to prevent cross-network proof replay. External queries are only needed for data availability, not trust in the validator set.

### 2.7 Trust Bootstrapping

For a fresh verifier to validate proofs, they must possess a trust anchor:

1. **Genesis trust**: Verifier knows the genesis validator set public keys
2. **Checkpoint chain**: Each checkpoint commits to the next validator set; verifier can trace from genesis
3. **Pinned checkpoint**: Verifier trusts a recent checkpoint obtained out-of-band (e.g., from a trusted source)

This is analogous to TLS certificate chains: the proof is self-contained, but root trust must be established externally. A proof URL is valid if it chains back to a checkpoint signed by ≥2/3 of validators in a trusted set.

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
weight = stakeWeight × (0.7 + 0.3 × ageWeight)
```

Where:
- `stakeWeight` = Account's staked balance / Total staked
- `ageWeight` = min(accountAgeDays, 365) / 365

**Key property:** Age weight is gated behind stake. An account with zero stake has zero weight regardless of age, preventing farming of old zero-stake accounts for consensus influence.

The age component is capped at 1 year to prevent early-adopter lock-in. Staked accounts earn a 0-30% bonus based on account age, rewarding long-term participation while ensuring stake remains the primary weight factor.

This creates Sybil resistance: new accounts with no stake have zero weight. Established, staked accounts anchor consensus.

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

1. **Leader Selection**: Randomized weighted selection proportional to stake. At height `h`, the leader is selected by:
   - Compute randomness seed: `seed = SHA256(prevCheckpointId || prevAggregatedBLSSignature)`
   - The aggregated BLS signature provides unpredictable entropy (hard to grind without controlling ≥67% stake)
   - Derive selection value: `v = (seed mod totalStake)`
   - Select validator whose cumulative stake threshold range contains `v`
   - This ensures validators lead checkpoints proportional to stake while preventing deterministic DoS targeting
   - **Last-signer influence caveat**: The final validator to submit their signature has marginal influence over the next seed. Mitigations include: (a) signature collection timeouts that exclude late signers, (b) leader fallback to next-in-line after timeout, (c) reduced weight for validators with high late-submission rates. Future work may explore commit-reveal schemes or VDF-based randomness for higher-stakes applications.

2. **Quorum Threshold**: A checkpoint is valid when signed by validators representing ≥ 2/3 of total staked weight.

3. **Fork Choice Rule**: Before finality, nodes follow the heaviest-weight chain. After checkpoint finalization, that branch becomes canonical.

4. **Conflicting Checkpoints**: If a validator signs conflicting checkpoints at the same height, they are slashed for double-signing (15% of stake).

5. **Validator Set Updates**: The active validator set is determined by stake positions at the previous checkpoint. Changes take effect at the next checkpoint boundary.

6. **Active Signing Committee**: While any account meeting minimum stake can register as a validator, each checkpoint is signed by an active committee selected from the validator pool:
   - **Committee size (N)**: 32-64 validators recommended for decentralization + manageable URL proof sizes
   - **QR mode**: For QR-compatible proofs, use smaller committees (N ≤ 21) with packed + multi-proof encoding
   - **Selection**: Top N validators by stake weight, or rotating selection using beacon randomness for larger pools
   - **Threshold (k)**: At least ⌈2N/3⌉ committee members must sign (e.g., k ≥ 43 for N = 64)
   - **Rotation**: Committee membership updates each checkpoint based on stake changes
   - With multi-proof optimization, proof size is ~O(N) rather than O(k · log₂N)

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

Expected performance: ~15-30s average finality under normal network conditions. Finality rate depends on validator availability and network partition tolerance. Testnet telemetry will inform production targets.

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

*Epoch 0 theoretical max (31.5M) exceeds cap; actual emission stops when `totalSupply >= 30,000,000 RKU`.

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
- **Proof Size:** 600-1,500 chars for typical proofs (1-5 depth ancestry fits in QR codes)

### 11.2 Scalability Path

- DAG structure enables horizontal scaling
- Checkpoint parallelization
- State sharding (future work)
- Layer 2 solutions for high-frequency use cases

## 12. Security Model

### 12.1 Assumptions

Rinku's security relies on:

1. **Honest stake majority**: ≥2/3 of staked weight is controlled by honest validators
2. **Cryptographic hardness**: ECDSA P-256 and SHA-256 remain secure
3. **Network synchrony**: Messages propagate within bounded time for liveness (not safety)
4. **Trust anchor availability**: Verifiers can obtain a valid genesis or checkpoint reference

### 12.2 What Each Proof Profile Guarantees

**Profile A (Receipt):**
- Sender cannot forge transaction signatures
- Transaction hash is tamper-evident
- Ancestry chain is internally consistent
- *Does NOT prove*: checkpoint was actually signed by validators (trusts signatureCount claim)

**Profile B (Full Finality):**
- All Profile A guarantees, plus:
- Merkle inclusion proves tx was in the checkpoint's transaction set
- Validator signatures prove ≥2/3 stake attested to the checkpoint
- Validator set commitment allows verification without knowing current validators

**Profile C (Self-Contained v5 - MerkleSumTree Multi-Proof):**
- All Profile B guarantees, plus:
- **Fully offline verification** - verifier derives totalWeight from MerkleSumTree, no trust assumptions beyond chain identity
- MerkleSumTree membership proofs embed signer data with cryptographic binding to totalWeight
- validatorSumTreeRoot (hash + totalWeight) is included in BLS signing hash, binding BOTH signer weights AND totalWeight to signature
- Verifier recomputes MerkleSumTree root from proofs, verifies it matches claimed root
- BLS aggregated signature (48 bytes) replaces individual ECDSA signatures (94.9% compression for 21 validators)
- *Does prove*: Signer weight sum AND totalWeight accuracy using MerkleSumTree cryptographic commitment
- *Closes denominator attack*: totalWeight cannot be forged because it's derived from the signed MerkleSumTree root

### 12.3 Attack Vectors and Mitigations

| Attack | Description | Mitigation |
|--------|-------------|------------|
| **Eclipse attack** | Isolate a node to feed false checkpoints | Multiple peer connections, checkpoint pinning |
| **Fake committee** | Attacker claims false signatureCount | Profile B verification; community checkpoint pins |
| **Long-range attack** | Attacker with old keys rewrites history | Checkpoint chain commitment to validator set changes |
| **Withheld data** | Sender provides proof but data unavailable | Receiver can request Profile B upgrade; network redundancy |
| **Leader targeting** | DoS specific checkpoint leaders | Randomized leader selection (beacon from aggregated BLS sig) |
| **Double-spend** | Conflicting transactions in DAG | Weight-based fork resolution; checkpoint finality locks |
| **Stake grinding** | Manipulate randomness for leader selection | Beacon uses aggregated BLS signature (requires ≥67% to grind) |

### 12.4 Profile A Trust Model

Profile A receipts are analogous to **signed receipts in traditional commerce**:
- The merchant trusts the payment network processed the transaction
- The receipt proves the customer authorized the payment
- Full audit requires contacting the payment processor

For most retail/P2P transactions, Profile A provides sufficient assurance. High-value or adversarial contexts should use Profile B or await additional checkpoint confirmations.

## 13. Conclusion

Rinku demonstrates that URLs can serve as the canonical portable representation of distributed ledger proofs. By encoding transactions, proofs, and ancestry in self-contained URLs, we eliminate infrastructure dependency for verification.

Through DEFLATE compression, complete finality proofs fit within 600-2,300 characters for typical ancestry depths (1-10 transactions). Single transactions and short ancestry chains (up to ~5 depth) fit in QR codes; complex DAG proofs work as shareable URLs in any modern browser. This enables genuine offline verification: embed payment proofs in QR codes, verify transactions without network access, share ledger state via hyperlinks.

The combination of DAG-based consensus, weighted proof-of-stake, checkpoint finality, and deflationary tokenomics creates a functional distributed ledger with a novel property: **the link is the proof**.

---

## References

1. Nakamoto, S. (2008). Bitcoin: A Peer-to-Peer Electronic Cash System.
2. Popov, S. (2018). The Tangle. IOTA Foundation.
3. Buterin, V. (2014). Ethereum: A Next-Generation Smart Contract Platform.
4. Castro, M. & Liskov, B. (1999). Practical Byzantine Fault Tolerance.

## Appendix A: Cryptographic Primitives

- **Signatures:** ECDSA P-256 with SHA-256 (chosen for native Web Crypto API support)
- **BLS Signatures:** BLS12-381 shortSignatures for checkpoint aggregation (48-byte G1 signatures, 96-byte G2 public keys) via @noble/curves
- **Hashing:** SHA-256 for transactions, Merkle trees, checkpoints, validator set commitments
- **Key Derivation:** 40-character fingerprint from first 20 bytes of SHA-256(public key)
- **Compression:** DEFLATE (pako) for URL payload encoding
- **Validator Key Storage:** AES-256-GCM encryption with scrypt key derivation (N=16384, r=8, p=1)

### A.1 Validator Identity Binding

Each validator maintains two keypairs:
1. **ECDSA keypair** - For transaction signing and identity (address derived from public key fingerprint)
2. **BLS keypair** - For checkpoint signature aggregation (registered during staking)

The binding is: `stake(amount, ecdsaPubKey, blsPubKey)` → validator entry includes both keys. Slashing applies to the staked amount regardless of which key misbehaves. This dual-key design allows efficient checkpoint aggregation (BLS) while maintaining transaction compatibility (ECDSA).

### A.2 Deterministic Encoding

For consensus determinism, all serialized values use:
- **Integers only**: Amounts, weights, fees, nonces are unsigned 64-bit integers in smallest units (no floating point)
- **Canonical JSON**: Keys sorted lexicographically, no whitespace, UTF-8 encoding
- **Big-endian byte order**: For all binary serializations (hashes, signatures)
- **Timestamps**: Unix milliseconds as integer

This prevents rounding divergence across implementations.

### A.3 MerkleSumTree Specification

The MerkleSumTree commits to both validator identity and aggregate weight using canonical byte encoding (no string concatenation ambiguity):

**Leaf node construction (byte encoding):**
```
leaf.hash = SHA256(
  0x00 ||                    // 1-byte domain separator (leaf)
  uint32_be(index) ||        // 4 bytes, big-endian
  address_bytes ||           // 20 bytes (raw, not hex)
  blsPubKey_bytes ||         // 96 bytes (raw G2 point)
  uint64_be(weight)          // 8 bytes, big-endian
)
leaf.sumWeight = weight
```

**Internal node construction (byte encoding):**
```
node.hash = SHA256(
  0x01 ||                    // 1-byte domain separator (internal)
  left.hash ||               // 32 bytes
  uint64_be(left.sumWeight) ||  // 8 bytes, big-endian
  right.hash ||              // 32 bytes
  uint64_be(right.sumWeight)    // 8 bytes, big-endian
)
node.sumWeight = left.sumWeight + right.sumWeight
```

**Root commitment:**
```
validatorSumTreeRoot = { hash: root.hash, totalWeight: root.sumWeight }
```

**Membership proof:** For each signer, the proof contains:
- `leaf`: { index (uint32), address (20 bytes), blsPublicKey (96 bytes), weight (uint64) }
- `siblings`: Array of { hash (32 bytes), sumWeight (uint64) } for each tree level
- `pathBits`: Boolean array indicating left (false) or right (true) at each level

**Verification:**
1. Recompute leaf hash from signer data using byte encoding above
2. Walk up tree using siblings and pathBits, applying internal node hash formula
3. Verify computed root matches claimed `validatorSumTreeRoot.hash`
4. Verify `totalWeight == validatorSumTreeRoot.totalWeight == root.sumWeight`

**Canonical encoding:** All integer fields use unsigned big-endian encoding. Address and public key fields are raw bytes (not hex strings). This ensures cross-implementation determinism.

The BLS signing hash includes the full validatorSumTreeRoot tuple (hash + totalWeight), binding both signer weights AND total denominator to the signature.

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

Self-Contained Proof URL (Profile C v4 - MerkleSumTree):
```
rinku://sp/{base64url(deflate(json({
  version: 4,
  txHash: string,
  txSignature: string,
  txFrom: string,
  txTo: string,
  txAmount: number,
  txNonce: number,
  txTimestamp: number,
  checkpointHeight: number,
  checkpointId: string,
  txMerkleRoot: string,
  stateRoot: string,
  receiptRoot: string,
  tipCount: number,
  merkleProof: string[],
  merkleIndex: number,
  blsAggregatedSig: string,      // base64url-encoded 48-byte G1 signature
  blsSignerBitmap: string,       // base64url-encoded bitmap
  blsSignerCount: number,
  signerMembershipProofs: [{     // MerkleSumTree membership proof per signer
    leaf: {
      index: number,
      address: string,
      blsPublicKey: string,      // base64url-encoded 96-byte G2 pubkey
      weight: number
    },
    siblings: [{ hash: string, sumWeight: number }],
    pathBits: boolean[]
  }],
  validatorSumTreeRoot: {        // Cryptographically binds totalWeight
    hash: string,
    totalWeight: number
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

Measured on reference implementation (vitest, Node.js) with high-entropy data:

```
Single transaction:     596 chars  (0.58 KB)
2-ancestor proof:       941 chars  (0.92 KB)
5-ancestor proof:     1,411 chars  (1.38 KB)
10-ancestor proof:    2,228 chars  (2.18 KB)
15-tx DAG proof:      3,709 chars  (3.62 KB)
31-tx DAG proof:      7,248 chars  (7.08 KB)
```

**QR Code compatibility (byte mode):**
- Single tx through 5-depth: fits QR-L/M (up to 2,331 bytes)
- 10-depth and beyond: requires URL sharing, not QR
- All proofs fit browser URL limits (65KB+)

**Benchmark methodology:**
- Random 40-char hex addresses (high entropy)
- Random 64-char hex hashes
- 88-char ECDSA signatures (realistic length)
- Checkpoint anchors include signatureCount (not full signatures array)

## Appendix E: BLS Signature Aggregation

### E.1 Motivation

Traditional ECDSA checkpoint signatures require O(n) storage for n validators. With 21 validators and 48-byte signatures each, this adds 1,008 bytes to every proof. BLS signature aggregation reduces this to a constant 48 bytes regardless of validator count.

### E.2 BLS12-381 Specification

Rinku uses BLS12-381 shortSignatures (G1 signatures, G2 public keys):
- **Private Key:** 32 bytes (random scalar)
- **Public Key:** 96 bytes (G2 point, compressed)
- **Signature:** 48 bytes (G1 point, compressed)
- **Aggregated Signature:** 48 bytes (constant, regardless of signer count)

Implementation via @noble/curves library, which provides:
- Constant-time operations for side-channel resistance
- Pure JavaScript with no native dependencies

### E.2.1 Proof-of-Possession (Rogue-Key Mitigation)

BLS signature aggregation is vulnerable to rogue-key attacks where an attacker chooses a malicious public key that cancels legitimate keys during aggregation. Rinku mitigates this via proof-of-possession:

1. **Registration requirement:** When staking, validators must submit a proof-of-possession: `PoP = BLS.sign(blsPrivateKey, blsPublicKey)`
2. **Verification at stake time:** The staking contract verifies `BLS.verify(blsPublicKey, blsPublicKey, PoP)` before accepting the validator
3. **Commitment in MerkleSumTree:** Only PoP-verified public keys are included in the validator sum tree

This ensures all aggregated public keys are honestly generated (the signer knows the corresponding private key), preventing rogue-key attacks on aggregate signature verification.

**Aggregate verification:** Given signer public keys `{pk_1, ..., pk_k}` and aggregated signature `σ_agg`:
1. Compute aggregate public key: `pk_agg = pk_1 + pk_2 + ... + pk_k` (G2 point addition)
2. Verify: `BLS.verify(pk_agg, message, σ_agg)`

This single-pairing verification is efficient and secure under PoP. The @noble/curves library is also WebAssembly-free for maximum portability.

### E.3 Compression Ratios

| Validators | ECDSA Total | BLS Aggregated | Compression |
|------------|-------------|----------------|-------------|
| 1 | 48 bytes | 48 bytes | 0% |
| 5 | 240 bytes | 48 bytes | 80% |
| 10 | 480 bytes | 48 bytes | 90% |
| 21 | 1,008 bytes | 48 bytes | 95.2% |
| 100 | 4,800 bytes | 48 bytes | 99% |

The 48-byte aggregated signature plus 3-byte signer bitmap enables QR-compatible finality proofs.

### E.4 Signing Process

1. **Checkpoint Creation:** Leader computes checkpoint header including txMerkleRoot, stateRoot, receiptRoot, tipCount
2. **Validator Set Commitment:** Build MerkleSumTree from validators (per Appendix A.3); extract `validatorSumTreeRoot = { hash, totalWeight }`
3. **Signing Hash:** Compute SHA-256 of: `chainId || checkpointId || height || txMerkleRoot || stateRoot || receiptRoot || tipCount || validatorSumTreeRoot.hash || validatorSumTreeRoot.totalWeight` (all fields in canonical byte encoding per A.2)
   - **Chain identity binding:** The `chainId` (4 bytes, network-specific constant) is the first field, preventing cross-network proof replay
4. **Individual Signatures:** Each validator signs the hash with their BLS private key
5. **Aggregation:** Combine all signatures into single 48-byte aggregated signature
6. **Bitmap:** Create signer bitmap indicating which validators signed

### E.5 Verification Process (Multi-Proof)

1. **Verify chain identity:** Confirm chainId/genesisHash in signing hash matches expected network
2. **Extract multi-proof:** Parse signer leaves and auxiliary nodes from proof payload
3. **Reconstruct MerkleSumTree:**
   - Place signer leaves at their bitmap indices in layer[0]
   - Place auxiliary nodes at specified (level, index) positions
   - Compute parents level-by-level: `parent = hash(left || right)`
4. **Verify root matches:** `layer[treeDepth][0] == validatorSumTreeRoot` (hash AND totalWeight)
5. **Recompute signing hash:** Using chainId + checkpoint fields + validatorSumTreeRoot (hash AND totalWeight)
6. **Extract signer public keys:** From multi-proof leaf data
7. **Compute aggregate public key:** `pk_agg = Σ pk_i` (sum signer BLS public keys via G2 point addition)
8. **Verify aggregated signature:** `BLS.verify(pk_agg, signingHash, σ_agg)` (single-pairing verification)
9. **Compute signer weight:** Sum weights from multi-proof leaves
10. **Check threshold:** Verify signerWeight ≥ 67% of derived totalWeight

Note: This verification is secure because: (a) all validator BLS public keys are PoP-verified at registration time (see E.2.1), preventing rogue-key attacks, and (b) chainId binding prevents proofs from one network being replayed on another.

### E.6 Security Properties

**MerkleSumTree Binding:** The validatorSumTreeRoot (containing both hash AND totalWeight) is included in the BLS signing hash. Any attempt to modify signer weights OR totalWeight invalidates the aggregated signature.

**Denominator Attack Prevention:** Unlike signer-only witness schemes, MerkleSumTree proves totalWeight cryptographically. The verifier recomputes the tree root from membership proofs and verifies totalWeight matches the signed commitment.

**Tamper Detection:** Verifiers recompute MerkleSumTree root from embedded membership proofs. Mismatched roots indicate tampering. Each membership proof cryptographically binds the signer's weight to the tree structure.

**Threshold Verification:** Verifiers compute `signerWeight / totalWeight` where:
- `signerWeight` = sum of weights from membership proof leaves (cryptographically verified)
- `totalWeight` = derived from MerkleSumTree root (cryptographically verified)
Both values are trustlessly derived, closing all weight forgery attack vectors.

### E.7 Validator Key Management

Validators maintain both ECDSA (transaction signing) and BLS (checkpoint signing) keypairs:

```
{
  ecdsaPrivateKey: Uint8Array,  // 32 bytes
  ecdsaPublicKey: Uint8Array,   // 65 bytes (uncompressed)
  blsPrivateKey: Uint8Array,    // 32 bytes
  blsPublicKey: Uint8Array      // 96 bytes (G2 compressed)
}
```

Keys are encrypted at rest using AES-256-GCM with scrypt-derived key (N=16384, r=8, p=1). Password required at node startup; development mode uses consistent default for testing.

## Appendix F: Profile C Packed Encoding

For QR compatibility, Profile C proofs require compact binary encoding instead of JSON.

### F.1 Size Estimates (JSON vs Packed)

| Component | JSON Encoding | Packed Binary |
|-----------|---------------|---------------|
| Transaction (typical) | ~300 bytes | ~150 bytes |
| Checkpoint header | ~400 bytes | ~200 bytes |
| Merkle proof (10 levels) | ~700 bytes | ~320 bytes |
| BLS aggregated sig | 64 bytes (base64) | 48 bytes |
| Signer bitmap (64 validators) | 12 bytes | 8 bytes |
| **Per-signer membership proof:** | | |
| - Leaf (index + addr + pubkey + weight) | ~220 bytes | 128 bytes |
| - Siblings (6 levels × {hash,weight}) | ~600 bytes | 240 bytes |
| - pathBits | ~20 bytes | 1 byte |
| **Total per signer** | ~840 bytes | ~369 bytes |

### F.2 Profile C Size Examples

**N = 32 committee, k = 22 signers, 6-level tree:**
- JSON encoding: ~300 + 400 + 350 + 64 + 4 + (22 × 840) = **~19,600 bytes** (URL only)
- Packed binary: ~150 + 200 + 160 + 48 + 4 + (22 × 369) = **~8,680 bytes** (URL only)
- Packed + DEFLATE: ~4,500-6,000 bytes (URL only)

**N = 21 committee, k = 14 signers, 5-level tree:**
- JSON encoding: ~300 + 400 + 280 + 64 + 3 + (14 × 700) = **~10,850 bytes** (URL only)
- Packed binary: ~150 + 200 + 128 + 48 + 3 + (14 × 300) = **~4,730 bytes** (URL only)
- Packed + DEFLATE: ~2,200-2,800 bytes (**QR-L feasible**)

### F.3 Packed Format Specification (v4 - Per-Signer Proofs)

*Note: This format is superseded by v5 (F.3.1) which uses multi-proofs for smaller payloads.*

```
Profile C Packed Format v4 (binary):

Header (fixed):
  version:        1 byte (0x04 for v4)
  chainId:        4 bytes (network identifier)
  txHash:         32 bytes
  txSig:          64 bytes (ECDSA)
  cpHeight:       varint
  txMerkleRoot:   32 bytes
  stateRoot:      32 bytes
  receiptRoot:    32 bytes
  tipCount:       varint
  
Merkle Proof:
  proofLength:    1 byte (number of levels)
  proofHashes:    proofLength × 32 bytes
  proofIndex:     varint

BLS Aggregation:
  aggSig:         48 bytes
  signerCount:    1 byte
  signerBitmap:   ceil(N/8) bytes

Validator Commitment:
  valRootHash:    32 bytes
  totalWeight:    8 bytes (uint64 BE)

Per-Signer Membership Proofs (repeated signerCount times):
  leafIndex:      varint
  leafAddress:    20 bytes
  leafBlsPubKey:  96 bytes
  leafWeight:     8 bytes (uint64 BE)
  siblingCount:   1 byte
  siblings:       siblingCount × 40 bytes (32B hash + 8B weight)
  pathBits:       ceil(siblingCount/8) bytes
```

### F.3.1 Packed Format Specification (v5 - Multi-Proof)

Multi-proof format shares sibling nodes across signers, reducing payload by 60-75%.

```
Profile C Packed Format v5 (binary):

Header (fixed):
  version:        1 byte (0x05 for v5)
  chainId:        4 bytes (network identifier - prevents cross-chain replay)
  txHash:         32 bytes
  txSig:          64 bytes (ECDSA)
  cpHeight:       varint
  txMerkleRoot:   32 bytes
  stateRoot:      32 bytes
  receiptRoot:    32 bytes
  tipCount:       varint
  
Transaction Merkle Proof:
  proofLength:    1 byte (number of levels)
  proofHashes:    proofLength × 32 bytes
  proofIndex:     varint

BLS Aggregation:
  aggSig:         48 bytes
  signerBitmap:   ceil(N/8) bytes (signer indices encoded as bitmap)

Validator Commitment:
  valRootHash:    32 bytes
  totalWeight:    8 bytes (uint64 BE)
  committeeSize:  1 byte (N, the total number of validators in committee)
  // Note: treeDepth = ⌈log₂N⌉ is derived from committeeSize, not transmitted

Multi-Proof Signer Leaves (signerCount derived from bitmap popcount):
  For each signer (in index order):
    leafAddress:    20 bytes
    leafBlsPubKey:  96 bytes
    leafWeight:     8 bytes (uint64 BE)

Multi-Proof Auxiliary Nodes:
  auxCount:       varint
  For each auxiliary node (sorted by level, then index):
    level:          1 byte
    index:          varint
    hash:           32 bytes
    sumWeight:      8 bytes (uint64 BE)
```

**Reconstruction Algorithm:**
1. Parse committeeSize (N) from proof header; compute treeDepth = ⌈log₂N⌉ (not transmitted, derived)
2. Initialize layer[0] with signer leaves at their bitmap indices (0 to N-1)
3. Place auxiliary nodes at specified (level, index) positions in canonical order (sorted by level ascending, then index ascending)
4. For each level from 0 to treeDepth-1:
   - For each pair (i, i+1) where i is even: if both children present, compute parent = hash(left || right)
   - If only left child present and index < ⌈layerSize/2⌉, use padding node {hash:"padding", sumWeight:0} for right
   - Place computed parent at layer[level+1][i/2]
5. Verify layer[treeDepth][0] == valRootHash (both hash AND sumWeight must match)

### F.4 QR Compatibility Matrix (Without Multi-Proof)

*Per-signer proofs (v4 format) - for reference:*

| Committee (N) | Signers (k) | JSON + DEFLATE | Packed + DEFLATE | QR-L (2,953B) |
|--------------|-------------|----------------|------------------|---------------|
| 16 | 11 | ~5,000 | ~2,500 | ✓ |
| 21 | 14 | ~6,800 | ~3,400 | ✗ |
| 32 | 22 | ~10,200 | ~5,100 | ✗ |
| 64 | 43 | ~21,500 | ~10,700 | ✗ |

### F.4.1 QR Compatibility Matrix (With Multi-Proof)

*Multi-proof format (v5) - recommended:*

| Committee (N) | Signers (k) | Packed v5 + DEFLATE | QR-L (2,953B) |
|--------------|-------------|---------------------|---------------|
| 16 | 11 | ~1,600 | ✓ |
| 21 | 14 | ~2,100 | ✓ |
| 32 | 22 | ~2,900 | ✗ |
| 64 | 43 | ~5,400 | ✗ |

**Conclusion:** With multi-proof (v5), QR codes are compatible with committees up to N ≤ 21. For N > 21, use URL sharing. The v5 format is the recommended default for all Profile C proofs.

### F.5 Merkle Multi-Proof Optimization

Individual membership proofs repeat many sibling nodes across the tree. A **multi-proof** (batch proof) shares common siblings, reducing proof size substantially when k is large.

**Complexity Analysis:**
- **Individual proofs:** `k × log₂N` sibling nodes (each signer carries full path)
- **Multi-proof:** `~N` nodes total (leaves + sparse auxiliary nodes not on any signer path)

**Savings by Committee Size:**

| N | k (⅔N) | Individual Nodes | Multi-Proof Nodes | Savings |
|---|--------|------------------|-------------------|---------|
| 16 | 11 | 44 | ~16 | **63.6%** |
| 21 | 14 | 70 | ~21 | **70.0%** |
| 32 | 22 | 110 | ~32 | **70.9%** |
| 64 | 43 | 258 | ~64 | **75.2%** |

**Multi-Proof Structure:**
```
MerkleSumMultiProof {
  leaves: MerkleSumLeaf[],      // k signer leaves
  auxiliaryNodes: {             // Non-signer nodes needed for reconstruction
    level: number,              // Tree level (0 = leaf level)
    index: number,              // Position at that level
    node: MerkleSumNode         // {hash, sumWeight}
  }[]
}
```

**Verification Algorithm:**
1. Place all k leaves at their indices in level 0
2. Place auxiliary nodes at their specified positions
3. Compute parent level: for each pair, hash children → parent
4. Repeat until root is computed
5. Verify computed root matches expected validatorSumTreeRoot

**Updated Size Estimates (Packed + Multi-Proof + DEFLATE):**

| N | k | Without Multi-Proof | With Multi-Proof | Improvement |
|---|---|---------------------|------------------|-------------|
| 16 | 11 | ~2,500 bytes | ~1,600 bytes | 36% smaller |
| 21 | 14 | ~3,400 bytes | ~2,100 bytes | 38% smaller |
| 32 | 22 | ~5,100 bytes | ~2,900 bytes | **43% smaller** |
| 64 | 43 | ~10,700 bytes | ~5,400 bytes | **50% smaller** |

**QR Compatibility with Multi-Proof:**

With multi-proof optimization, QR compatibility extends to:
- **N ≤ 21** (vs N ≤ 16 without): ~2,100 bytes fits QR-L (2,953 bytes)
- Larger committees remain URL-only but significantly more compact

**Security:** Multi-proofs provide identical security guarantees—the verifier still reconstructs the full MerkleSumTree root and verifies all signer weights are cryptographically bound.
