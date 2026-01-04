# Rinku: A URL-Native Distributed Ledger

**Abstract.** A distributed ledger where URLs serve as the canonical portable representation of transactions, proofs, and state transitions. Self-contained proofs embedded in URLs enable trustless verification without trusted infrastructure - data availability is provided by the sender or network. We propose a DAG-based consensus mechanism with weight-based Sybil resistance combining stake and account age. The system achieves per-transaction finality through periodic checkpoints, implements deflationary tokenomics with halving-based emission, supports extensible smart contracts, and provides optional zero-knowledge privacy through QR-compatible ZK URLs that prove payment validity without revealing transaction details. The result is a ledger where the link itself is the proof.

## 1. Introduction

Traditional blockchains require nodes to verify state. Users must trust infrastructure providers or run their own nodes. We eliminate the need to trust infrastructure for verification by encoding the complete verification path in URLs - data availability is provided by the sender or network, but trust is not required.

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
  amount: uint64,    // Transfer amount (smallest units, per A.2)
  fee: uint64,       // Gas fee (smallest units)
  nonce: uint64,     // Sender's sequence number
  tipUrls: string[], // References to DAG tips (0-2 parents)
  ts: uint64,        // Unix timestamp (milliseconds)
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

DEFLATE compression achieves ~40-55% reduction on transaction JSON. Compression gains come primarily from repeated field names and structural redundancy (keys like "from", "to", "amount" appear in every transaction), even when values are high-entropy hex strings (e.g signatures).

### 2.4 Platform Compatibility

QR codes require byte mode for base64url encoding (alphabet contains `-` and `_` instead of `+` and `/`):

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

**Profile C: Self-Contained Proof (MerkleSumTree with Multi-Proof)**
- Size: 1,600-2,100 bytes (packed binary + DEFLATE) for N ≤ 21; ~2,100-2,800 URL chars after base64url encoding; QR-compatible for committees N ≤ 21, URL-shareable for larger
- URL format: `rinku://sp/{base64url-deflate-packed}` (canonical scheme; HTTP URLs are transport wrappers)
- Includes: chainId (network identifier), tx data, full checkpoint header, Merkle inclusion proof, BLS aggregated signature (48 bytes), MerkleSumTree multi-proof (signer leaves + shared auxiliary nodes), validatorSumTreeRoot (hash + totalWeight)
- Guarantees: **Fully offline verification** - verifier derives totalWeight from MerkleSumTree root
- Trust assumption: **Chain identity only** - verifier must know expected chainId; totalWeight is cryptographically bound to validator set via MerkleSumTree; BLS public keys are PoP-verified at registration (see E.2.1)
- Security: validatorSumTreeRoot (containing both hash and totalWeight) is included in BLS signing hash, preventing both signer weight forgery AND denominator attacks
- Use case: Offline verification, air-gapped systems, cross-chain bridges, legal evidence
- **Why multi-proof optimization?** Without multi-proof, each signer requires a full Merkle path: `O(k · log₂N)` nodes. With multi-proof, sibling nodes are shared across signers, reducing to `~O(N)` total nodes (60-75% smaller). This is critical for QR compatibility:
  
  | Committee (N) | Signers (k) | Without Multi-Proof | With Multi-Proof | Savings |
  |---------------|-------------|---------------------|------------------|---------|
  | 16 | 11 | ~44 nodes | ~16 nodes | 64% |
  | 21 | 14 | ~63 nodes | ~21 nodes | 67% |
  | 32 | 22 | ~110 nodes | ~32 nodes | 71% |
  
  - **QR-compatible** (≤2,953 bytes): Requires packed binary + multi-proof AND committee N ≤ 21
  - **URL-shareable** (always): All proofs fit browser URL limits (65KB+) with any reasonable committee size
  - **Packed encoding required for QR**: Raw bytes for all fields (no JSON overhead). See Appendix B.1
  - **Recommendation**: Default to URL sharing; QR codes viable for committees N ≤ 21 with packed + multi-proof

Most use cases are served by Profile A receipts. Profile B provides full finality when validator set is known. **Profile C is the gold standard** - completely self-contained proofs with cryptographically-bound totalWeight, enabling true offline verification without any trust assumptions beyond the cryptographic primitives.

> **Why Multi-Proof Verification Works (No Full Validator List Needed)**
>
> A common question: "How can you verify without all validator public keys?"
>
> The answer: multi-proof reconstruction yields the same `validatorSumTreeRoot` (hash + totalWeight) as if you had the full validator list. Here's why:
>
> 1. **Signer public keys are in the proof** - The k signers' BLS public keys are included as leaf data, sufficient for BLS aggregate verification
> 2. **Non-signer contributions are captured in auxiliary nodes** - Internal nodes commit to the aggregate hash and sumWeight of their subtrees, so non-signer weights are included in the root without needing their individual public keys
> 3. **Root reconstruction is deterministic** - Given signer leaves + auxiliary nodes, there's exactly one way to rebuild the tree and compute the root
> 4. **Denominator is cryptographically bound** - The `totalWeight` in `validatorSumTreeRoot` is verified by reconstruction, not trusted from the proof; and the entire tuple is signed by validators, preventing forgery
>
> Result: The verifier knows (a) the exact signers who attested, (b) their individual weights, and (c) the total committee weight - all without transmitting non-signer public keys.

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

The URL carries the proof. Profile A requires trust in the checkpoint source. Profile B requires a known validator set. Profile C is self-contained for *finality verification* - but chain identity binding is required to prevent cross-network proof replay. External queries are only needed for data availability, not trust in the validator set.

### 2.7 Trust Bootstrapping

For a fresh verifier to validate proofs, they must possess a trust anchor:

1. **Genesis trust**: Verifier knows the genesis validator set public keys
2. **Checkpoint chain**: Each checkpoint commits to the next validator set; verifier can trace from genesis
3. **Pinned checkpoint**: Verifier trusts a recent checkpoint obtained out-of-band (e.g from a trusted source)

This is analogous to TLS certificate chains: the proof is self-contained, but root trust must be established externally. A proof URL is valid if it chains back to a checkpoint signed by ≥2/3 of validators in a trusted set.

## 3. Zero-Knowledge Privacy

Rinku extends its URL-native philosophy to privacy with ZK URLs - portable zero-knowledge proofs that enable **selective disclosure** of payment validity without revealing transaction details to proof recipients.

### 3.1 The Selective Disclosure Problem

Public ledgers inherently expose transaction details. Even with pseudonymous addresses, payment patterns, amounts, and counterparty relationships are visible on-chain. For many use cases, users need to prove they made a payment without revealing full transaction details to the proof recipient (even though the underlying chain data remains transparent to those who query it).

Existing privacy solutions require:
- Full chain privacy (Zcash) - requires complete protocol rewrite
- Trusted mixers - introduces counterparty risk
- Layer 2 solutions - adds complexity and withdrawal delays

Rinku's approach: **selective disclosure at the proof layer**. The base chain remains transparent; ZK URLs hide transaction details *from the proof recipient* while cryptographically proving the payment exists.

### 3.2 ZK URL Format

Privacy-preserving proofs use the `rinku://zk/` scheme:

```
rinku://zk/{base64url(deflate(payload))}
```

A ZK URL proves that:
1. A valid transaction exists in a finalized checkpoint
2. The prover knows the transaction details and authorized them
3. The proof is bound to the correct chain

**Without revealing to the proof recipient:**
- Sender address
- Recipient address  
- Transaction amount
- Which specific transaction in the Merkle tree

> **Important:** This is selective disclosure, not full privacy. The underlying transaction remains visible on the transparent base chain. The ZK URL hides details from someone who *only has the URL* - useful for receipts, payment confirmations, and contexts where you want to prove payment without sharing your wallet address.

### 3.3 How It Works

The ZK layer uses Groth16 SNARKs with the following flow:

```
┌─────────────┐     ┌──────────────┐     ┌─────────────┐
│ Transaction │ ──► │ Merkle Proof │ ──► │ ZK Circuit  │
│  (private)  │     │  (private)   │     │  (proves)   │
└─────────────┘     └──────────────┘     └─────────────┘
                                                │
                                                ▼
                                         ┌─────────────┐
                                         │ ZK URL      │
                                         │ (shareable) │
                                         └─────────────┘
```

**Prover (sender):**
1. Fetch Merkle witness from any node for their finalized transaction
2. Generate ZK proof using their private key seed (derives BabyJubJub keypair)
3. Encode proof as `rinku://zk/...` URL
4. Share URL via QR code, message, or any medium

**Verifier (recipient/anyone):**
1. Receive ZK URL
2. Decode and verify Groth16 proof (~10ms, offline*)
3. Check nullifier against local cache (prevents double-claims in same context)
4. Accept payment proof without learning transaction details

*\*Offline verification assumes the verifier has the circuit verification key pinned by hash (distributed with wallets/apps). See Section 3.8 for trust model.*

### 3.4 Cryptographic Design

The ZK circuit (10-level Poseidon Merkle tree, ~10.5k constraints) proves:

| Constraint | Purpose |
|------------|---------|
| Merkle inclusion | Transaction exists in checkpoint |
| EdDSA signature | Prover authorized the transaction |
| Nullifier derivation | Unique per transaction, prevents replay |
| Amount commitment | Pedersen commitment hides value |
| Chain binding | Prevents cross-chain replay attacks |

**Key insight:** The circuit uses EdDSA on BabyJubJub (ZK-friendly curve) rather than the ledger's ECDSA P-256. Users derive a separate ZK keypair from a seed phrase. This separation allows the base ledger to remain efficient while enabling optional privacy.

### 3.5 Security Properties

| Property | Mechanism |
|----------|-----------|
| **Transaction privacy** | ZK proof hides which tx in Merkle tree (selective disclosure) |
| **Amount privacy** | Pedersen commitment with random blinding |
| **Sender/recipient privacy** | Only commitments revealed to proof recipient |
| **Double-claim prevention** | Nullifier uniqueness - local cache or on-chain registry |
| **Replay protection** | Chain ID hash bound in circuit |
| **Offline verification** | All data in URL; requires pinned verification key |

> **Nullifier Semantics:** The nullifier prevents the same proof from being reused. Two deployment modes exist:
> 1. **Receipt-only (offline):** Verifier maintains local nullifier cache (POS terminal, wallet). Protects against replay *in the same context* but not globally.
> 2. **Global uniqueness (online):** Nullifiers published to on-chain registry or accumulator. Requires eventual online step but provides network-wide double-claim protection.

### 3.6 Performance

| Metric | Value |
|--------|-------|
| Proof generation | 2-3 seconds (server-side) |
| Proof verification | <10 milliseconds |
| URL length | ~500-800 characters |
| QR compatibility | Version 15 (fits with room to spare) |

### 3.7 Use Cases

1. **Private payments** - Prove you paid someone without revealing your wallet
2. **Confidential payroll** - Employees verify salary without exposing amounts to others
3. **Anonymous airdrops** - Claim rewards without linking identity
4. **Offline verification** - Scan QR at point-of-sale, verify without internet
5. **Privacy-preserving receipts** - Legal proof of payment without full disclosure

### 3.8 Trust Model

The ZK layer adds minimal trust assumptions beyond the base protocol:

1. **Trusted setup** - Groth16 requires a structured reference string (SRS) from a Powers of Tau ceremony. Verifiers must trust the ceremony was conducted correctly (at least one honest participant). The verification key hash is distributed with wallets.
2. **Verification key** - Offline verification requires the circuit's verification key pinned by hash. Wallets distribute this key; verifiers accept proofs only for known key hashes. This is analogous to pinning TLS certificate authorities.
3. **Nullifier tracking** - Receipt-only mode (local cache) provides context-specific protection. Global double-claim prevention requires publishing nullifiers on-chain or querying a nullifier accumulator.
4. **ZK keypair security** - User must protect their seed phrase.

The base chain remains fully transparent. Privacy is opt-in per proof via selective disclosure, enabling regulatory compliance while offering user choice.

### 3.9 Related Work

The ZK URL concept builds on established cryptographic patterns:

- **Payment disclosure / viewing keys** (Zcash) - Selective disclosure of shielded transactions. Rinku adapts this spirit to URL-portable proofs.
- **Commitment + nullifier schemes** (Tornado Cash, Semaphore) - Nullifier patterns for double-spend prevention. Rinku uses similar derivation but emphasizes offline-first verification.
- **Self-contained proofs** (recursive SNARKs, IBC) - Portable validity proofs. Rinku's contribution is the URL-native packaging with explicit QR/size budgets.

**What's novel:** The specific combination of (1) proof-as-a-link as the canonical UX object, (2) offline verification emphasis with explicit trust anchors, (3) QR-compatible size constraints, and (4) integration with checkpoint finality proofs.

See **Appendix H** for detailed implementation specifications including circuit constraints, API endpoints, and proof encoding.

## 4. DAG-Based Consensus

### 4.1 Structure

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

### 4.2 Conflict Resolution

When conflicting transactions exist (e.g double-spend attempts), the transaction with greater cumulative weight wins. Weight flows from tips backward through the DAG.

### 4.3 Weight Calculation

Transaction weight derives from the originating account's weighted proof-of-stake:

```
weightBps = (stakeBalance * 10000 / totalStaked) * (7000 + 3000 * ageBps / 10000) / 10000
```

Where (all fixed-point integers in basis points, 1 bps = 0.01%):
- `stakeBalance` = Account's staked balance (uint64, smallest units)
- `totalStaked` = Network total staked balance (uint64)
- `ageBps` = min(accountAgeDays * 10000 / 365, 10000) - age factor in basis points, capped at 10000 (1 year)
- Integer division truncates toward zero (floor rounding)

Equivalent floating-point formula (for clarity only): `weight = stake% × (0.7 + 0.3 × ageWeight)`

**Key property:** Age weight is gated behind stake. An account with zero stake has zero weight regardless of age, preventing farming of old zero-stake accounts for consensus influence.

The age component is capped at 1 year to prevent early-adopter lock-in. Staked accounts earn a 0-30% bonus based on account age, rewarding long-term participation while ensuring stake remains the primary weight factor.

This creates Sybil resistance: new accounts with no stake have zero weight. Established, staked accounts anchor consensus.

### 3.4 Age Weight Mitigations

To prevent gaming of the age component:
- **Capped duration**: Age weight saturates at 365 days
- **Log-scale consideration**: Future versions may apply logarithmic scaling
- **Staked duration**: Alternative metric measuring continuous stake time rather than account creation

## 5. Checkpoints and Finality

### 11.1 Checkpoint Creation

Every 15 seconds (configurable), the network produces a checkpoint:

```
{
  id: string,           // SHA-256 hash of header fields
  height: uint32,       // Sequential checkpoint number
  timestamp: uint64,    // Creation time (Unix ms)
  txMerkleRoot: string, // Merkle root of transaction hashes
  stateRoot: string,    // Merkle root of account states
  receiptRoot: string,  // Merkle root of execution receipts
  tipCount: uint8,      // Number of DAG tips at checkpoint
  previousId: string,   // Link to prior checkpoint
  txHashes: string[],   // Transactions in this checkpoint
  signatures: string[], // Validator signatures
  signatureCount: uint8
}
```

**Checkpoint ID Computation (byte-level):**

The `id` field is computed deterministically from header fields. Two independent implementations MUST produce identical IDs from identical input:

```
checkpointId = SHA256(
  "rinku:checkpoint:v1" ||     // 19 bytes, ASCII domain separator
  uint32_be(chainId) ||         // 4 bytes, network identifier
  uint32_be(height) ||          // 4 bytes
  uint64_be(timestamp) ||       // 8 bytes, Unix milliseconds
  previousId_bytes ||           // 32 bytes (raw SHA-256, or 32 zero bytes for genesis)
  txMerkleRoot_bytes ||         // 32 bytes (raw SHA-256)
  stateRoot_bytes ||            // 32 bytes (raw SHA-256)
  receiptRoot_bytes ||          // 32 bytes (raw SHA-256)
  uint8(tipCount)               // 1 byte
)
```

**Field encoding:**
- All integers use unsigned big-endian encoding
- Hash fields are raw 32-byte SHA-256 digests (not hex strings)
- Domain separator is UTF-8 encoded ASCII
- Genesis checkpoint uses 32 zero bytes for `previousId`
- Total preimage size: 19 + 4 + 4 + 8 + 32 + 32 + 32 + 32 + 1 = 164 bytes

Note: `txHashes`, `signatures`, and `signatureCount` are NOT included in `id` computation. The ID commits only to header fields; signatures are collected afterward and may vary across nodes during aggregation.

### 11.2 Consensus Protocol

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

6. **Active Signing Committee**: While any account meeting minimum stake can register as a validator (supporting large pools of hundreds/thousands), each checkpoint is signed by a bounded active committee selected from the validator pool:
   - **Validator pool**: Unlimited registrations; all staked validators earn proportional rewards
   - **Committee size (N)**: Bounded to 32-64 for URL proof sizes (default: 64); ≤21 for QR mode
   - **Selection**: Top N validators by stake weight at checkpoint h-1
   - **Rotation via beacon**: For pools larger than N, use rotating selection: `selectedIndices = beaconShuffle(seed, poolSize).slice(0, N)` where `seed = SHA256(prevCheckpointId || prevAggBLSSig)`. This prevents ossification into "top-N forever" while maintaining stake-weighted influence
   - **Index Assignment**: Committee indices (0 to N-1) are assigned by sorting selected validators deterministically: `(stakeWeight DESC, ecdsaFingerprint ASC)`. This ensures all nodes build identical MerkleSumTree roots
   - **Threshold (k)**: At least ⌈2N/3⌉ committee members must sign (e.g k ≥ 43 for N = 64)
   - With multi-proof optimization, proof size is ~O(N) rather than O(k · log₂N)

### 5.3 Finality

A transaction achieves finality when included in a checkpoint with sufficient validator signatures. The checkpoint's Merkle roots commit to all included transactions and resulting state. Once finalized:
- The transaction cannot be reversed
- Proof URLs are bounded to checkpoint ancestry
- State is frozen at that point

### 5.4 Finality Metrics

The network tracks:
- Average time-to-finality
- Median and P95 finality times
- Pending transaction count
- Checkpoint latency
- Throughput (tps)

Expected performance: ~10-15s average finality under normal network conditions. Finality rate depends on validator availability and network partition tolerance. Testnet telemetry will inform production targets.

## 6. State Management

### 11.1 Account Model

Each account maintains:

```
{
  fingerprint: string,    // 40-char public key hash
  balance: uint64,        // Current balance (smallest units)
  nonce: uint64,          // Transaction counter
  firstTxTimestamp: uint64 // Account creation time (Unix ms)
}
```

### 11.2 State Transitions

Transactions modify state atomically:
1. Verify sender has sufficient balance (amount + fee)
2. Decrement sender balance by (amount + fee)
3. Increment recipient balance by amount
4. Increment sender nonce
5. Process fee (50% burn, 50% to validators)

### 5.3 Merkle State Proofs

Account state is committed to a Merkle tree with root included in each checkpoint (`stateRoot`). Any account balance can be proven with O(log n) proof size. Proofs are anchored to checkpoint state roots for self-contained verification.

## 7. Tokenomics

### 11.1 Base Unit

All on-chain values use the smallest indivisible unit:
- **1 RKU = 1,000,000 µRKU** (micro-RKU)
- All amounts, fees, rewards, and supply values are `uint64` in µRKU
- Human-readable RKU values are for documentation only; implementations use µRKU

### 11.2 Supply

- **Maximum Supply:** 30,000,000 RKU = 30,000,000,000,000 µRKU (hard cap, enforced)
- **Genesis Allocation:** 6,000,000 RKU
  - 3,000,000 RKU - Treasury
  - 2,000,000 RKU - Staking rewards reserve
  - 1,000,000 RKU - Faucet distribution (testnet only)
- **Emission:** Up to 24,000,000 RKU via checkpoint rewards

### 8.3 Emission Schedule

Rewards halve every 3,150,000 checkpoints (~18 months at 15s intervals):

| Epoch | Checkpoints | Reward (µRKU) | Reward (RKU) | Epoch Emission | Cumulative |
|-------|-------------|---------------|--------------|----------------|------------|
| 0 | 0 – 3,149,999 | 3,932,411 | 3.932411 | 12,387,094.65 RKU | 12,387,095 RKU |
| 1 | 3,150,000 – 6,299,999 | 1,966,205 | 1.966205 | 6,193,545.75 RKU | 18,580,641 RKU |
| 2 | 6,300,000 – 9,449,999 | 983,102 | 0.983102 | 3,096,771.30 RKU | 21,677,412 RKU |
| 3 | 9,450,000 – 12,599,999 | 491,551 | 0.491551 | 1,548,385.65 RKU | 23,225,798 RKU |
| 4 | 12,600,000 – 15,749,999 | 245,775 | 0.245775 | 774,191.25 RKU | 23,999,989 RKU |
| 5+ | 15,750,000+ | 122,887 | 0.122887 | floor until cap | ≤24,000,000 RKU |

**Derivation (see Appendix C for authoritative constants):**
```
emissionPool = 24,000,000,000,000 µRKU
epochMultiplier = 3,150,000 × 1.9375 = 6,103,125
initialReward = emissionPool // epochMultiplier = 3,932,411 µRKU
reward(epoch) = max(minReward, initialReward >> epoch)
minReward = initialReward >> 5 = 122,887 µRKU
```

*Note: `//` = integer division, `>>` = right shift (floor division by 2^n). Emission halts when cumulative reaches cap. ~11 RKU headroom remains after epoch 4; epoch 5+ operates at floor rate until exhausted.*

**Hard Cap Enforcement**: Once total circulating supply reaches 30,000,000 RKU, checkpoint rewards drop to 0. The floor reward of 122,887 µRKU only applies while supply remains below the cap.

### 8.4 Halving Rationale

The 18-month halving interval (vs. Bitcoin's 4 years) balances:
- **Sustained validator incentives:** Validators remain rewarded over multi-year timeframes
- **Gradual transition:** Smooth progression from emission to fee-based economics
- **Long-term sustainability:** Avoids emission cliff-dive during network growth phase
- **Predictable schedule:** Complete emission over ~7.5 years

### 7.5 Reward Distribution

Checkpoint rewards distributed to active validators using Weighted Proof-of-Stake:
- **70% proportional to stake amount**
- **30% proportional to effective account age**

**Anti-Gaming Measures:**
- Age weight requires minimum bonded stake (100 RKU) to qualify
- Missed checkpoints decay age weight by 10% per miss
- Encourages consistent participation and discourages grinding strategies

This rewards both capital commitment and long-term, active participation.

### 7.6 Adaptive Fee Split

Gas fees are split between validators and burn using an adaptive model:

```
progressiveBurn = min(BURN_CEILING, (circulatingSupply / MAX_SUPPLY) / SUPPLY_TARGET × BURN_CEILING)

where:
  circulatingSupply = GENESIS_ALLOCATION + totalEmitted - totalBurned
  
  Note: GENESIS_ALLOCATION (6M RKU) includes treasury (3M), staking reserve (2M), 
  and faucet (1M). All are counted as "circulating" for fee split calculation - treasury funds are not excluded because they may enter circulation at any time.
  
  BURN_CEILING = 30%
  SUPPLY_TARGET = 50%
  MAX_SUPPLY = 30,000,000 RKU

validatorShare = max(70%, 100% - progressiveBurn)
burnShare = 100% - validatorShare
```

**progressiveBurn Examples:**
| Circulating Supply | Supply Ratio | progressiveBurn | Validator Share |
|-------------------|--------------|-----------------|-----------------|
| 6M RKU (genesis) | 20% | 12% | 88% |
| 9M RKU | 30% | 18% | 82% |
| 12M RKU | 40% | 24% | 76% |
| 15M+ RKU | ≥50% | 30% (cap) | 70% (floor) |

**Key Properties:**
- **Validator Floor:** Validators always receive at least 70% of fees
- **Progressive Burn:** LINEAR function - burn percentage scales linearly from genesis to 50% supply target
- **Supply-Aware:** Prioritizes validator incentives during early growth phase
- **Deflationary Pressure:** Net supply decreases when burn rate exceeds emission rate

This ensures validators are adequately compensated for signing Profile C proofs, especially during high-demand periods when their workload increases.

## 8. Dynamic Gas Fees (EIP-1559 Style)

### 11.1 Pricing Model

Rinku uses an EIP-1559-inspired pricing mechanism that adjusts based on **utilization vs target**, not paid fees. This prevents runaway feedback loops where high fees compound into higher fees.

```
if (txsThisPeriod > targetTxs):
    baseFee = baseFee × (1 + changePercent)
else:
    baseFee = baseFee × (1 - changePercent)

changePercent = min(12.5%, |utilization - 1| / elasticity)
```

**Key Parameters:**
- **Target:** 15 transactions per 15-second checkpoint period (~1 TPS baseline)
- **Max change:** ±12.5% per period (prevents spikes > ~3× in 10 periods)
- **Elasticity:** 2× (price increases maximally when load is 2× target)

**Bounds (in µRKU):**
- Minimum: 1,000 µRKU (0.001 RKU)
- Maximum: 10,000,000 µRKU (10 RKU)

### 11.2 Self-Correcting Behavior

Unlike fee-averaging models that compound:
- **Under target load:** Price decreases 12.5% per period until minimum
- **At target load:** Price remains stable
- **Above target load:** Price increases 12.5% per period toward maximum

This ensures fees remain affordable for URL-sized receipts while still providing spam resistance during high demand.

### 8.3 Fee Validation

Transactions must include a fee meeting current minimum. Insufficient fees result in rejection. Priority is first-come-first-served at the current base fee - no fee auctions or tip bidding.

## 9. Staking and Slashing

### 11.1 Staking

Any account can stake RKU to become a validator:
1. Lock tokens in staking contract
2. Gain weight in consensus
3. Earn proportional checkpoint rewards
4. Subject to slashing for misbehavior

Minimum stake: 100 RKU

### 11.2 Slashing Penalties

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

## 10. Smart Contracts (Work in Progress)

### 11.1 Architecture

Contracts are URL-encoded programs with:
- Immutable code (WASM bytecode)
- Mutable state (key-value storage)
- Defined interface (callable methods)

### 11.2 Execution Model

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

### 10.3 Current Status

The contract framework is implemented with:
- Deploy, call, and query interfaces
- State persistence
- Gas metering hooks

Full WASM execution is under development. Current implementation uses a simulated runtime for interface validation.

## 11. Network Protocol

### 11.1 Peer Discovery

Nodes discover peers via gossip protocol:
1. Exchange known peer lists
2. Validate peer liveness
3. Maintain connection pool (max 50 peers)

### 11.2 State Synchronization

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

## 12. Performance

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

## 13. Security Model

### 13.1 Assumptions

Rinku's security relies on:

1. **Honest stake majority**: ≥2/3 of staked weight is controlled by honest validators
2. **Cryptographic hardness**: ECDSA P-256 and SHA-256 remain secure
3. **Network synchrony**: Messages propagate within bounded time for liveness (not safety)
4. **Trust anchor availability**: Verifiers can obtain a valid genesis or checkpoint reference

### 13.2 What Each Proof Profile Guarantees

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

### 13.3 Attack Vectors and Mitigations

| Attack | Description | Mitigation |
|--------|-------------|------------|
| **Eclipse attack** | Isolate a node to feed false checkpoints | Multiple peer connections, checkpoint pinning |
| **Fake committee** | Attacker claims false signatureCount | Profile B verification; community checkpoint pins |
| **Long-range attack** | Attacker with old keys rewrites history | Checkpoint chain commitment to validator set changes |
| **Withheld data** | Sender provides proof but data unavailable | Receiver can request Profile B upgrade; network redundancy |
| **Leader targeting** | DoS specific checkpoint leaders | Randomized leader selection (beacon from aggregated BLS sig) |
| **Double-spend** | Conflicting transactions in DAG | Weight-based fork resolution; checkpoint finality locks |
| **Stake grinding** | Manipulate randomness for leader selection | Beacon uses aggregated BLS signature (requires ≥67% to grind) |

### 13.4 Profile A Trust Model

Profile A receipts are analogous to **signed receipts in traditional commerce**:
- The merchant trusts the payment network processed the transaction
- The receipt proves the customer authorized the payment
- Full audit requires contacting the payment processor

For most retail/P2P transactions, Profile A provides sufficient assurance. High-value or adversarial contexts should use Profile B or await additional checkpoint confirmations.

## 14. Conclusion

Rinku demonstrates that URLs can serve as the canonical portable representation of distributed ledger proofs. By encoding transactions, proofs, and ancestry in self-contained URLs, we eliminate the need to trust infrastructure for verification.

Through DEFLATE compression, complete finality proofs fit within 600-2,300 characters for typical ancestry depths (1-10 transactions). Single transactions and short ancestry chains (up to ~5 depth) fit in QR codes; complex DAG proofs work as shareable URLs in any modern browser. This enables genuine offline verification: embed payment proofs in QR codes, verify transactions without network access, share ledger state via hyperlinks.

The ZK privacy layer extends this paradigm to confidential payments. Users can generate `rinku://zk/` URLs that prove payment validity without revealing sender, recipient, or amount - all in ~500-800 characters that fit in a QR code. Privacy becomes opt-in at the proof layer rather than requiring chain-wide changes.

The combination of DAG-based consensus, weighted proof-of-stake, checkpoint finality, deflationary tokenomics, and zero-knowledge privacy creates a functional distributed ledger with a novel property: **the link is the proof**.

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

Note: The `sumWeight` is included in the hash preimage (not just metadata), ensuring weight commitments are cryptographically bound at every tree level. The `0x00`/`0x01` domain separators prevent second-preimage attacks between leaf and internal nodes.

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

**Non-Power-of-Two Committee Padding:**

When committee size N is not a power of two, the tree requires canonical padding for missing right children. Use the deterministic `EMPTY_NODE`:

```
EMPTY_NODE = {
  hash: SHA256("rinku:empty_node:v1"),   // Domain-separated, deterministic
  sumWeight: 0
}
```

During tree construction, if a node at position `2i` has no sibling at `2i+1`, use `EMPTY_NODE` as the right child. This ensures all implementations compute identical roots regardless of committee size.

**EMPTY_NODE Semantics:**
- `EMPTY_NODE` is ONLY valid for right-side padding beyond `committeeSize` (non-power-of-two cases)
- It is NOT a general wildcard for missing nodes; any "real" missing node causes proof rejection
- A prover cannot exploit `EMPTY_NODE` because substituting it for a real validator would produce a different root hash
- Verifiers: use `EMPTY_NODE` only when `rightIdx >= committeeSize` at level 0, or when the right subtree is structurally empty due to tree geometry

**Tree Geometry:**
- Tree depth: `⌈log₂N⌉` where N is committee size
- Layer size at level `l`: `⌈N / 2^l⌉`
- Builder and verifier MUST use identical layer size calculations

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
    height: uint32,
    signatureCount: uint8
  }
})))}
```

### B.1 Self-Contained Proof URL (Profile C)

Profile C uses packed binary encoding for compact size. JSON encoding is available for debugging but does not fit QR codes.

**Packed Binary (Canonical):**
```
rinku://sp/{base64url(deflate(packed_binary))}
```

**Packed binary layout (byte-level):**
```
version:            1 byte   (0x01, packed binary format)
chainId:            4 bytes  (uint32_be, network identifier)
txHash:            32 bytes  (raw SHA-256)
txSignature:       64 bytes  (raw ECDSA P-256)
txFrom:            20 bytes  (raw address)
txTo:              20 bytes  (raw address)
txAmount:           8 bytes  (uint64_be, µRKU)
txFee:              8 bytes  (uint64_be, µRKU)
txNonce:            8 bytes  (uint64_be)
txTimestamp:        8 bytes  (uint64_be, Unix ms)
checkpointHeight:   4 bytes  (uint32_be)
checkpointId:      32 bytes  (raw SHA-256)
txMerkleRoot:      32 bytes  (raw SHA-256)
stateRoot:         32 bytes  (raw SHA-256)
receiptRoot:       32 bytes  (raw SHA-256)
tipCount:           1 byte   (uint8)
merkleProofDepth:   1 byte   (uint8, 0-20)
merkleProof:       32 × depth bytes (raw hashes, bottom-up)
merkleIndex:        2 bytes  (uint16_be)
blsAggregatedSig:  48 bytes  (raw G1 point)
committeeSize:      1 byte   (uint8, N)
signerBitmap:      ⌈N/8⌉ bytes (packed bits, LSB-first)
signerCount:        1 byte   (uint8, k = popcount(bitmap))
multiProof:        variable  (see below)
validatorSumTreeRoot.hash:        32 bytes
validatorSumTreeRoot.totalWeight:  8 bytes (uint64_be)
```

**Multi-proof layout:**
```
signerLeaves: k × (1 + 20 + 96 + 8) bytes per signer
  - index:        1 byte  (uint8, committee position) [see note below]
  - address:     20 bytes (raw)
  - blsPublicKey: 96 bytes (raw G2 point)
  - weight:       8 bytes (uint64_be)

NOTE: The `index` field is technically redundant since signerLeaves are
ordered by ascending index and indices are derivable from signerBitmap
via popcount walk. Retained for debugging/sanity checks. Implementations
MAY omit index (saving k bytes) by deriving from bitmap; if so, use
version byte 0x02 to distinguish.

auxiliaryNodeCount: 1 byte (uint8)
auxiliaryNodes: count × (1 + 1 + 32 + 8) bytes per node
  - level:    1 byte (uint8, 0 = leaf layer)
  - index:    1 byte (uint8, position at that level)
  - hash:    32 bytes (raw SHA-256)
  - sumWeight: 8 bytes (uint64_be)
```

Multi-proof shares sibling nodes across signers, achieving ~O(N) total vs O(k·log₂N) for individual proofs. Tree depth is derived from `committeeSize`: `treeDepth = ⌈log₂(committeeSize)⌉`.

**Canonical Multi-Proof Constraints:**

Verifiers MUST reject proofs violating these rules:

1. **Sorted auxiliary nodes**: `auxiliaryNodes` MUST be sorted lexicographically by `(level, index)` in ascending order
2. **No duplicates**: No two auxiliary nodes may share the same `(level, index)` position
3. **No overlap with signers**: Auxiliary node positions at level 0 must not overlap any signer leaf index (signer leaves are authoritative at those positions)
4. **Complete reconstruction**: Reconstruction must not encounter missing nodes at required positions; only the explicit `EMPTY_NODE` (per A.3) is valid for non-power-of-two padding at specific right-child positions
5. **Minimal auxiliary set**: Every auxiliary node MUST be consumed exactly once during reconstruction; reject proofs with unused nodes (prevents padding attacks)
6. **Count consistency**: `auxiliaryNodeCount` MUST equal the array length; reconstruction must consume exactly `count` entries
7. **Consistent signer ordering**: `signerLeaves` MUST be ordered by ascending `index` (matching bitmap bit order)

These constraints ensure a single canonical encoding per proof, preventing malleability attacks where different encodings verify identically but compare differently. The "minimal + consumed" rules (5-6) specifically close off attacks where provers append ignored auxiliary nodes to create distinct byte encodings of the same logical proof.

**Index Semantics:**

All tree indices are 0-based, left-to-right:
- **Leaf indices** (level 0): 0 to N-1, corresponding to committee positions
- **Internal node indices**: At level `l`, indices range from 0 to `⌈N/2^l⌉ - 1`
- **merkleIndex** (transaction Merkle tree): Position of txHash in the checkpoint's transaction list, 0-based

**Committee Size Limits:**

The packed binary format uses `uint8` for `committeeSize`, `signerCount`, and auxiliary `(level, index)` fields. This imposes hard limits:
- Maximum committee size: 255 validators
- Maximum signers per proof: 255
- Maximum tree depth: 8 levels (2^8 = 256 leaves)

For committees larger than 255, a future format revision with varint or uint16 fields would be required. Current recommendation: committees ≤64 for URL proofs, ≤21 for QR.

**JSON Encoding (Debug Only):**
```
rinku://sp/{base64url(deflate(json({
  version: 1,
  chainId: uint32,
  txHash: string,
  txSignature: string,
  txFrom: string,
  txTo: string,
  txAmount: uint64,
  txNonce: uint64,
  txTimestamp: uint64,
  checkpointHeight: uint32,
  checkpointId: string,
  txMerkleRoot: string,
  stateRoot: string,
  receiptRoot: string,
  tipCount: uint8,
  merkleProof: string[],
  merkleIndex: uint16,
  blsAggregatedSig: string,
  blsSignerBitmap: string,
  blsSignerCount: uint8,
  signerMembershipProofs: [{
    leaf: { index: uint8, address: string, blsPublicKey: string, weight: uint64 },
    siblings: [{ hash: string, sumWeight: uint64 }],
    pathBits: boolean[]
  }],
  validatorSumTreeRoot: { hash: string, totalWeight: uint64 }
})))}
```

JSON encoding is human-readable for debugging but does NOT fit in QR codes due to field name overhead. Implementations MUST support packed binary for production use.

## Appendix C: Genesis Configuration

All values in µRKU (1 RKU = 1,000,000 µRKU) or basis points where noted. This is the **authoritative source of truth** - narrative tables are derived from these constants.

```json
{
  "maxSupply": 30000000000000,
  "genesisAllocation": {
    "treasury": 3000000000000,
    "stakingReserve": 2000000000000,
    "faucet": 1000000000000
  },
  "initialReward": 3932411,
  "halvingInterval": 3150000,
  "minReward": 122887,
  "emissionStopsAtCap": true,
  "checkpointInterval": 15000,
  "unbondingPeriod": 1209600000,
  "quorumThresholdBps": 6700,
  "ageWeightCap": 365,
  "validatorFeeFloorBps": 7000,
  "burnCeilingBps": 3000,
  "supplyTargetForFullBurnBps": 5000,
  "minBondForAgeWeight": 100000000,
  "ageDecayPerMissBps": 1000
}
```

**Derivation of `initialReward`:**
```
emissionPool = maxSupply - sum(genesisAllocation) = 24,000,000,000,000 µRKU
epochMultiplier = halvingInterval × (1 + 1/2 + 1/4 + 1/8 + 1/16) = 3,150,000 × 1.9375 = 6,103,125
initialReward = emissionPool // epochMultiplier = 3,932,411 µRKU
minReward = initialReward >> 5 = 122,887 µRKU

(// = integer division, >> = right shift = floor division by 2^n)
```

**Units:**
- `maxSupply`, `genesisAllocation.*`, `initialReward`, `minReward`, `minBondForAgeWeight`: µRKU (uint64)
- `*Bps`: basis points (7000 = 70.00%, uint16)
- `halvingInterval`: checkpoints (uint32)
- `checkpointInterval`, `unbondingPeriod`: milliseconds (uint64)
- `ageWeightCap`: days (uint16)

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

1. **Registration requirement:** When staking, validators must submit a domain-separated proof-of-possession:
   ```
   popMessage = SHA256("RINKU_POP_V1" || blsPublicKey)
   PoP = BLS.sign(blsPrivateKey, popMessage)
   ```
2. **Verification at stake time:** The staking contract verifies `BLS.verify(blsPublicKey, popMessage, PoP)` before accepting the validator
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

### F.3 Packed Format History

*Per-signer proofs (without multi-proof optimization) are not recommended for production use due to size. They are documented here for comparison only.*

Per-signer proofs repeat full Merkle paths for each signer, resulting in `O(k × log₂N)` sibling nodes. Multi-proof (F.3.1) reduces this to `~O(N)` by sharing siblings.

**Recommendation:** Use multi-proof format (B.1) for all Profile C proofs.

### F.3.1 Packed Format Specification (Multi-Proof)

Multi-proof format shares sibling nodes across signers, reducing payload by 60-75%.

**Canonical format defined in Appendix B.1** - this section provides additional context.

Key design choices:
- **All fields fixed-width** (no varints in canonical format) for simpler parsing
- `checkpointHeight`: 4 bytes (uint32_be) - sufficient for billions of checkpoints
- `merkleIndex`: 2 bytes (uint16_be) - supports up to 65,535 txs per checkpoint
- `auxiliaryNodes[].index`: 1 byte (uint8) - committee capped at 255

See B.1 for byte-level layout. The canonical format uses version byte `0x01`.

**Reconstruction Algorithm (Deterministic Sparse-Map):**

The verifier reconstructs the tree using a layer-by-layer sparse map:

1. Parse `committeeSize` (N) from proof header; compute `treeDepth = ⌈log₂N⌉`
2. Initialize `layers[0..treeDepth]` as empty maps (level → index → node)
3. Place signer leaves at `layers[0][bitmap_index]` for each signer (derived from bitmap)
4. Place auxiliary nodes at `layers[aux.level][aux.index]` in canonical order (sorted by level ascending, then index ascending)
5. For each level `l` from 0 to `treeDepth-1`:
   - Compute `layerSize = ⌈N / 2^l⌉`
   - For each parent index `p` in 0..⌈layerSize/2⌉-1:
     - `leftIdx = 2p`, `rightIdx = 2p + 1`
     - `left = layers[l].get(leftIdx)` - MUST be present (error if missing)
     - `right = rightIdx >= layerSize ? EMPTY_NODE : layers[l].get(rightIdx)` - EMPTY_NODE only for structural padding
     - Compute `parent = { hash: H(left, right), sumWeight: left.sumWeight + right.sumWeight }`
     - Place at `layers[l+1][p]`
6. Verify `layers[treeDepth][0] == valRootHash` (both hash AND sumWeight must match)

Where `EMPTY_NODE = { hash: SHA256("rinku:empty_node:v1"), sumWeight: 0 }` per Appendix A.3.

**Invariant:** At each level, the verifier MUST have sufficient nodes (from signer leaves or auxiliary nodes) to compute all required parents. If a left child is missing, the proof is invalid.

### F.4 QR Compatibility Matrix (Without Multi-Proof)

*Per-signer proofs (v4 format) - for reference:*

| Committee (N) | Signers (k) | JSON + DEFLATE | Packed + DEFLATE | QR-L (2,953B) |
|--------------|-------------|----------------|------------------|---------------|
| 16 | 11 | ~5,000 | ~2,500 | ✓ |
| 21 | 14 | ~6,800 | ~3,400 | ✗ |
| 32 | 22 | ~10,200 | ~5,100 | ✗ |
| 64 | 43 | ~21,500 | ~10,700 | ✗ |

### F.4.1 QR Compatibility Matrix (With Multi-Proof)

*With multi-proof optimization (recommended):*

| Committee (N) | Signers (k) | Compressed (bytes) | URL chars* | QR-L (2,953B) |
|--------------|-------------|--------------------|-----------:|---------------|
| 16 | 11 | ~1,600 | ~2,155 | ✓ |
| 21 | 14 | ~2,100 | ~2,820 | ✓ |
| 32 | 22 | ~2,900 | ~3,890 | ✗ |
| 64 | 43 | ~5,400 | ~7,220 | ✗ |

*URL chars = `rinku://sp/` (11 chars) + base64url(compressed) ≈ 11 + ⌈bytes × 4/3⌉

**Conclusion:** With multi-proof optimization, QR codes are compatible with committees up to N ≤ 21. For N > 21, use URL sharing. Multi-proof is the recommended default for all Profile C proofs.

**Protocol Committee Limits:** The packed format uses uint8 for committeeSize, signerCount, and auxiliary (level, index) fields. This intentionally caps committees at 255 validators with tree depth ≤8. This aligns with the product goal of portable URL/QR proofs - larger committees would exceed size budgets. Future format revisions may use varints if needed.

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
    level: uint8,               // Tree level (0 = leaf level)
    index: uint8,               // Position at that level
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

**Security Guarantees (Explicit Checks):**

Multi-proofs provide identical security to individual proofs. Verifiers MUST perform all of:

1. **BLS signature verification** - Aggregate signature verifies against signer BLS public keys (from proof leaves)
2. **Quorum threshold** - `signerWeight / totalWeight ≥ quorumThresholdBps / 10000` (67% by default)
3. **Root reconstruction** - Computed `validatorSumTreeRoot` (hash AND totalWeight) matches the committed value
4. **Checkpoint binding** - The `validatorSumTreeRoot` tuple is part of the BLS signing hash, preventing substitution

A proof passes if and only if all four checks succeed. The multi-proof format changes only the encoding of signer membership proofs, not the security model.

## Appendix G: Normative Test Vectors

These test vectors enable cross-implementation verification. All values are deterministic.

### G.1 MerkleSumTree Test Vector (N=4 Committee)

**Committee (4 validators):**
```
Validator 0:
  index:     0x00
  address:   0x0000000000000000000000000000000000000001
  blsPubKey: 0x{96 bytes of 0x01}
  weight:    1000000 (1 RKU = 1,000,000 µRKU)

Validator 1:
  index:     0x01
  address:   0x0000000000000000000000000000000000000002
  blsPubKey: 0x{96 bytes of 0x02}
  weight:    2000000

Validator 2:
  index:     0x02
  address:   0x0000000000000000000000000000000000000003
  blsPubKey: 0x{96 bytes of 0x03}
  weight:    1500000

Validator 3:
  index:     0x03
  address:   0x0000000000000000000000000000000000000004
  blsPubKey: 0x{96 bytes of 0x04}
  weight:    500000
```

**Leaf Hash Computation (Validator 0):**
```
preimage = 0x00 ||                        // domain separator (leaf)
           0x00000000 ||                  // index (uint32_be)
           0x0000000000000000000000000000000000000001 ||  // address (20 bytes)
           0x{96 bytes of 0x01} ||        // blsPubKey (96 bytes)
           0x00000000000f4240             // weight (uint64_be = 1000000)
leafHash[0] = SHA256(preimage)            // 129-byte preimage
```

**Expected Tree Structure:**
```
Level 0 (leaves):
  [0]: { hash: leafHash[0], sumWeight: 1000000 }
  [1]: { hash: leafHash[1], sumWeight: 2000000 }
  [2]: { hash: leafHash[2], sumWeight: 1500000 }
  [3]: { hash: leafHash[3], sumWeight: 500000 }

Level 1 (internal):
  [0]: hash = SHA256(0x01 || leaf[0].hash || 0x00000000000f4240 || leaf[1].hash || 0x00000000001e8480)
       sumWeight = 3000000  // 81-byte preimage
  [1]: hash = SHA256(0x01 || leaf[2].hash || 0x000000000016e360 || leaf[3].hash || 0x000000000007a120)
       sumWeight = 2000000

Level 2 (root):
  [0]: hash = SHA256(0x01 || node[0].hash || 0x00000000002dc6c0 || node[1].hash || 0x00000000001e8480)
       sumWeight = 5000000

validatorSumTreeRoot = { hash: <computed>, totalWeight: 5000000 }
```

### G.2 Multi-Proof Test Vector (k=3 of N=4)

**Scenario:** Validators 0, 1, 3 sign (indices 0, 1, 3). Validator 2 abstains.

**Signer Bitmap:** `0b00001011` = `0x0B` (bits 0, 1, 3 set)

**Multi-Proof Structure:**
```
signerLeaves: [
  { index: 0, address: 0x...01, blsPubKey: 0x{96×0x01}, weight: 1000000 },
  { index: 1, address: 0x...02, blsPubKey: 0x{96×0x02}, weight: 2000000 },
  { index: 3, address: 0x...04, blsPubKey: 0x{96×0x04}, weight: 500000 }
]

auxiliaryNodes: [
  { level: 0, index: 2, hash: leafHash[2], sumWeight: 1500000 }
]
```

**Reconstruction Steps:**
1. Place leaves 0, 1, 3 at level 0 positions 0, 1, 3
2. Place auxiliary node at level 0 position 2
3. Level 0 → Level 1:
   - Parent[0] = hash(leaf[0], leaf[1]), sumWeight = 3000000
   - Parent[1] = hash(leaf[2], leaf[3]), sumWeight = 2000000
4. Level 1 → Level 2:
   - Root = hash(parent[0], parent[1]), sumWeight = 5000000
5. Verify: reconstructed root == validatorSumTreeRoot

**Signer Weight Calculation:**
```
signerWeight = 1000000 + 2000000 + 500000 = 3500000
totalWeight  = 5000000
ratio        = 3500000 / 5000000 = 70%
quorumThresholdBps = 6700 (67%)
PASS: 70% >= 67%
```

### G.3 Packed Binary Test Vector

**For the G.2 proof, packed multi-proof section (hex):**
```
03                              // signerCount = 3
00 0000...01 {96×01} 00000000000f4240  // signer 0: index(1B), addr(20B), pk(96B), weight(8B)
01 0000...02 {96×02} 00000000001e8480  // signer 1
03 0000...04 {96×04} 000000000007a120  // signer 3
01                              // auxiliaryNodeCount = 1
00 02 {leafHash[2]} 000000000016e360   // aux: level(1B), index(1B), hash(32B), weight(8B)
```

**Total multi-proof section size:**
- Signers: 3 × (1 + 20 + 96 + 8) = 375 bytes
- Aux nodes: 1 + 1 × (1 + 1 + 32 + 8) = 43 bytes
- Total: 418 bytes

Note: The packed format uses 1-byte index for signers (matching bitmap position), while the hash preimage uses 4-byte index (uint32_be) per A.3. This is intentional: packed format optimizes for size, hash preimage ensures determinism with fixed-width fields.

### G.4 Implementation Checklist

Cross-implementation testing should verify:
- [ ] Leaf hash matches for each validator (domain separator included)
- [ ] Internal node hash matches at each level
- [ ] Root hash and totalWeight match expected values
- [ ] Multi-proof reconstruction produces identical root
- [ ] Signer weight sum matches expectation
- [ ] Canonical constraints reject malformed proofs:
  - [ ] Unsorted auxiliary nodes → REJECT
  - [ ] Duplicate auxiliary nodes → REJECT
  - [ ] Unused auxiliary nodes → REJECT
  - [ ] Missing required nodes → REJECT

### G.5 Non-Power-of-Two Test Vector (N=5)

Tests EMPTY_NODE padding behavior for committees that aren't powers of two.

**Committee (5 validators):**
```
Validator 0: weight = 1,000,000 µRKU
Validator 1: weight = 1,000,000 µRKU
Validator 2: weight = 1,000,000 µRKU
Validator 3: weight = 1,000,000 µRKU
Validator 4: weight = 1,000,000 µRKU
```

**Tree Structure (depth = ⌈log₂5⌉ = 3):**
```
Level 0 (leaves):
  [0]: leaf0, sumWeight = 1,000,000
  [1]: leaf1, sumWeight = 1,000,000
  [2]: leaf2, sumWeight = 1,000,000
  [3]: leaf3, sumWeight = 1,000,000
  [4]: leaf4, sumWeight = 1,000,000
  [5]: EMPTY_NODE (right-pad, idx >= committeeSize)
  [6]: EMPTY_NODE
  [7]: EMPTY_NODE

Level 1:
  [0]: hash(leaf0, leaf1), sumWeight = 2,000,000
  [1]: hash(leaf2, leaf3), sumWeight = 2,000,000
  [2]: hash(leaf4, EMPTY_NODE), sumWeight = 1,000,000
  [3]: hash(EMPTY_NODE, EMPTY_NODE), sumWeight = 0

Level 2:
  [0]: hash(node1[0], node1[1]), sumWeight = 4,000,000
  [1]: hash(node1[2], node1[3]), sumWeight = 1,000,000

Level 3 (root):
  [0]: hash(node2[0], node2[1]), sumWeight = 5,000,000

validatorSumTreeRoot = { hash: <computed>, totalWeight: 5,000,000 }
```

**Key verification points:**
- EMPTY_NODE only appears at indices ≥ 5 (committeeSize)
- Subtrees containing only EMPTY_NODEs still get computed (sumWeight = 0)
- Final totalWeight = sum of real validators only

### G.6 Negative Test Vectors (MUST Reject)

Implementations MUST reject these malformed proofs:

**1. Bitmap/Signer Mismatch:**
```
signerBitmap: 0b00001111  (bits 0,1,2,3 set → expects 4 signers)
signerLeaves: [leaf0, leaf1, leaf2]  (only 3 leaves)
REJECT: signerCount (popcount) != len(signerLeaves)
```

**2. Auxiliary Nodes Unsorted:**
```
auxiliaryNodes: [
  { level: 1, index: 0, ... },
  { level: 0, index: 5, ... }  // level 0 < level 1 (out of order)
]
REJECT: auxiliaryNodes not sorted by (level, index) ascending
```

**3. Duplicate Auxiliary Position:**
```
auxiliaryNodes: [
  { level: 0, index: 2, hash: A, sumWeight: 100 },
  { level: 0, index: 2, hash: B, sumWeight: 200 }  // same position
]
REJECT: duplicate (level, index) in auxiliaryNodes
```

**4. Auxiliary Overlaps Signer Leaf:**
```
signerBitmap: 0b00000101  (signers at indices 0, 2)
auxiliaryNodes: [
  { level: 0, index: 2, ... }  // index 2 is already a signer
]
REJECT: auxiliary node at level 0 overlaps signer leaf index
```

**5. Unused Auxiliary Node:**
```
// N=4 committee, signers 0,1,2,3 (all sign)
signerBitmap: 0b00001111
auxiliaryNodes: [
  { level: 0, index: 5, ... }  // index 5 doesn't exist in N=4 tree
]
REJECT: auxiliary node not consumed during reconstruction
```

**6. Quorum Below Threshold:**
```
signerWeight = 3,000,000 µRKU
totalWeight  = 5,000,000 µRKU
ratio = 60%
quorumThresholdBps = 6700 (67%)
REJECT: 60% < 67%, quorum not met
```

**7. Root Hash Mismatch:**
```
// Proof claims validatorSumTreeRoot.hash = 0xABCD...
// But reconstruction produces 0x1234...
REJECT: reconstructed root hash != claimed validatorSumTreeRoot.hash
```

**8. Root totalWeight Mismatch:**
```
// Proof claims validatorSumTreeRoot.totalWeight = 10,000,000
// But reconstruction yields sumWeight = 5,000,000
REJECT: reconstructed totalWeight != claimed totalWeight
```

**9. Missing Left Child:**
```
// During reconstruction, need node at (level=0, index=2)
// Neither signerLeaves nor auxiliaryNodes provide it
// And index 2 < committeeSize, so EMPTY_NODE invalid
REJECT: required node missing (not EMPTY_NODE eligible)
```

**10. Invalid BLS Signature:**
```
// All structural checks pass, but:
blsVerify(aggregatedSig, signingHash, aggregatedPubKey) = false
REJECT: BLS aggregate signature verification failed
```

## Appendix H: ZK Privacy Implementation Details

This appendix provides detailed implementation specifications for the ZK Privacy layer described in Section 3. For conceptual overview, see the main section.

### H.1 Circuit Implementation

The privacy circuit is implemented in Circom with the following specifications:
- **Constraints:** ~10,500
- **Curve:** BN128 (alt_bn128)
- **Proof system:** Groth16
- **Hash function:** Poseidon (circomlib)
- **Signature scheme:** EdDSA-Poseidon on BabyJubJub (circomlibjs)

### H.2 URL Format

```
rinku://zk/{base64url(deflate(payload))}
```

Where `payload` is a JSON object:

```json
{
  "v": 1,                          // Version
  "chainId": "rinku-testnet",      // Chain binding
  "cpHeight": 12345,               // Checkpoint height
  "proof": "...",                  // Groth16 proof (JSON or base64)
  "publicInputs": {
    "cpRoot": "...",               // Checkpoint Merkle root (public signal 0)
    "nullifier": "...",            // Prevents double-claim (public signal 1)
    "amountCommitment": "...",     // Pedersen commitment to amount (public signal 2)
    "chainIdHash": "..."           // Poseidon hash of chainId (public signal 3)
  },
  "encryptedMemo": "...",          // For recipient only (optional)
  "auxData": {                     // For reconstruction
    "validatorRoot": "...",
    "totalWeight": 5000000
  }
}
```

### H.3 Cryptographic Primitives

| Component | Choice | Rationale |
|-----------|--------|-----------|
| ZK System | Groth16 (bn128) | Smallest proofs (~192 bytes), fast verification |
| Hash (circuit) | Poseidon | ZK-friendly, ~8x faster than SHA-256 in circuits |
| Commitments | Pedersen | Additively homomorphic, efficient in ZK |
| Signatures (circuit) | EdDSA-Poseidon (BabyJubJub) | Native to ZK circuits via circomlibjs |
| Curve | BabyJubJub | Embedded curve in bn128, efficient for EdDSA in Groth16 |
| Encryption | ECIES (Curve25519) | For encrypted memos to recipient (future) |

**Implementation Note:** The EdDSA signatures in the ZK circuit use BabyJubJub (a different curve from the ledger's ECDSA P-256). This is intentional - the ZK layer is an optional privacy overlay with its own keypair derived from a user-provided seed.

### H.4 Circuit Design

The ZK circuit proves knowledge of a valid transaction without revealing it:

**Private Witness (known only to prover):**
- Full transaction data (from, to, amount, fee, nonce, ts, sig)
- Merkle inclusion path (Poseidon hash)
- Sender private key
- Recipient viewing key
- Blinding factors for commitments

**Public Inputs (4 signals, matching H.2 JSON schema):**
- `cpRoot` - Checkpoint Merkle root (public signal 0)
- `nullifier` - Prevents double-claiming same proof (public signal 1)
- `amountCommitment` - Pedersen commitment to amount (public signal 2)
- `chainIdHash` - Poseidon hash of chainId for replay protection (public signal 3)

> **Design choice:** The ZK circuit proves Merkle inclusion and authorization. Checkpoint finality is proven *outside* the circuit via the existing Profile C self-contained proof (MerkleSumTree + BLS aggregated signature). This keeps the ZK circuit small (~10.5k constraints) and avoids the complexity of BLS verification inside SNARKs.

**Circuit Constraints:**

```
1. MERKLE_INCLUSION:
   // Prove tx exists in checkpoint without revealing which one
   poseidonMerkleVerify(txHash, merklePath, cpRoot) = true

2. SIGNATURE_VALID:
   // Prove transaction was signed by a valid key
   eddsaVerify(txData, txSig, senderPubKey) = true

3. NULLIFIER_DERIVATION:
   // Deterministic nullifier prevents double-claims
   nullifier = poseidon(senderSecret, cpHeight, txHash)

4. AMOUNT_COMMITMENT:
   // Commit to amount with blinding factor
   amountCommitment = pedersenCommit(amount, blindingFactor)

5. RECIPIENT_BINDING:
   // Bind proof to intended recipient
   recipientCommitment = poseidon(recipientViewKey)

6. CHAIN_BINDING:
   // Prevent cross-chain replay
   chainIdHash = poseidon(chainId)
```

### H.5 Security Properties

| Property | Mechanism |
|----------|-----------|
| **Transaction Privacy** | ZK proof hides which tx in Merkle tree (selective disclosure) |
| **Amount Privacy** | Pedersen commitment hides value from proof recipient |
| **Sender Privacy** | Proof doesn't reveal sender address to proof recipient |
| **Recipient Privacy** | Only commitment revealed; decrypt with view key |
| **Double-Claim Prevention** | Nullifier - local cache (offline) or on-chain registry (global) |
| **Replay Protection** | chainIdHash bound in circuit public inputs |
| **Offline Verification** | Requires pinned verification key hash (distributed with wallets) |

> **Note on selective disclosure:** These properties apply to the proof recipient. The underlying transaction remains visible on the transparent base chain to anyone who queries it.

### H.6 Verification Algorithm

```
function verifyZkProof(url: string, options: VerifyOptions): ZkVerifyResult {
  // 0. Verify we have a trusted verification key
  if (!TRUSTED_VKEY_HASHES.includes(hash(VERIFYING_KEY))) {
    return { valid: false, reason: 'Unknown verification key' };
  }

  // 1. Decode URL
  const payload = inflate(base64urlDecode(url.split('/')[2]));
  
  // 2. Verify Groth16 proof (ZK circuit: Merkle inclusion + authorization)
  if (!groth16Verify(payload.proof, payload.publicInputs, VERIFYING_KEY)) {
    return { valid: false, reason: 'Invalid ZK proof' };
  }
  
  // 3. Verify chain binding (prevent cross-chain replay)
  const expectedChainIdHash = poseidon(EXPECTED_CHAIN_ID);
  if (payload.publicInputs.chainIdHash !== expectedChainIdHash) {
    return { valid: false, reason: 'Wrong chain' };
  }
  
  // 4. Check nullifier (context-dependent)
  // - Receipt-only mode: check local cache (offline)
  // - Global mode: check on-chain registry (requires network)
  if (options.nullifierCache?.has(payload.publicInputs.nullifier)) {
    return { valid: false, reason: 'Nullifier already used in this context' };
  }
  
  // 5. (Optional) Verify checkpoint finality via Profile C proof
  // The cpRoot in public inputs should match a finalized checkpoint
  // This step requires the Profile C certificate in auxData
  if (options.requireFinality && payload.auxData?.profileCProof) {
    if (!verifyProfileC(payload.auxData.profileCProof, payload.publicInputs.cpRoot)) {
      return { valid: false, reason: 'Checkpoint not finalized' };
    }
  }
  
  return {
    valid: true,
    amountCommitment: payload.publicInputs.amountCommitment,
    cpHeight: payload.cpHeight,
    // Recipient can decrypt memo with their view key
  };
}
```

> **Architecture note:** The ZK circuit proves Merkle inclusion and transaction authorization. Checkpoint finality is optionally verified via a Profile C self-contained proof bundled in `auxData`. This separation keeps the ZK circuit small while preserving full offline finality verification when needed.

### H.7 Recipient Flow

For the recipient to understand a ZK payment:

1. Receive `rinku://zk/...` URL
2. Verify ZK proof (offline)
3. Decrypt `encryptedMemo` using their viewing key
4. Memo reveals: actual amount, sender identity (optional), payment reference
5. Optionally register nullifier to prevent proof reuse

### H.8 Proof Size Analysis

| Component | Size |
|-----------|------|
| Groth16 proof | 192 bytes |
| Public inputs | ~256 bytes |
| Encrypted memo | ~64 bytes |
| Auxiliary data | ~128 bytes |
| **Total (uncompressed)** | ~640 bytes |
| **Total (DEFLATE + base64)** | ~500 chars |

Fits comfortably in QR Code Version 15 (1,156 chars) with room to spare.

### H.9 Implementation Status

**Completed Features:**
- Circom circuit with 10-level Poseidon Merkle tree (~10.5k constraints)
- EdDSA-Poseidon signature verification (BabyJubJub via circomlibjs)
- Pedersen amount commitments with random blinding
- Chain ID binding (prevents cross-chain replay)
- Nullifier derivation (prevents double-claims)
- Trusted setup using Powers of Tau with crypto.randomBytes entropy

**Artifacts:**
- Circuit WASM: 3.14 MB
- Proving key (zkey): 6.71 MB
- Verification key: 3.39 KB

**API Endpoints:**
- `GET /api/zk/status` - ZK layer status and feature availability
- `GET /api/zk/witness/:txHash` - Fetch Merkle witness for finalized tx
- `POST /api/zk/prove` - Generate full ZK proof (accepts txHash, optional privateKeySeed)
- `POST /api/zk/verify` - Verify a `rinku://zk/...` URL

**Explorer Integration:**
- ZK Privacy tab with witness generation, proof generation, and verification
- Private key seed input for user-controlled ZK keypairs
- 24 integration tests passing

**Future Enhancements:**
- Client-side proof generation (WASM in browser, ~20-30s)
- Encrypted memos for recipient-only data
- Amount range proofs (prove amount > X without revealing exact value)
- Stealth addresses for enhanced recipient privacy

### H.10 Comparison with Alternatives

| Approach | Proof Size | Verification | Prover Time | Chain Changes |
|----------|------------|--------------|-------------|---------------|
| **Rinku ZK URL** | ~500-800 chars | <10ms | 2-3s (server) | None |
| Full ZK chain (Zcash) | N/A | N/A | 40s+ | Complete rewrite |
| STARKs | ~50KB | 50ms | 1-2s | None |
| Bulletproofs | ~700 bytes | 100ms | 10s | None |

**Benchmark (actual):** Proof generation averages 2.3s on server with snarkjs. Verification is <10ms. URL length ~800 chars fits QR Code Version 15.

Groth16 offers the best balance for URL-native QR-compatible proofs.

### H.11 Trust Assumptions

1. **Trusted Setup**: Groth16 requires a trusted setup ceremony. Use MPC (Powers of Tau + circuit-specific) to minimize trust.
2. **Nullifier Registry**: If using on-chain registry, privacy is reduced (reveals nullifier usage pattern). Wallet-level tracking provides full privacy but requires recipient cooperation.
3. **Viewing Keys**: Recipients must safeguard viewing keys; compromise reveals payment history.

### H.12 Future Work

- **Recursive proofs**: Aggregate multiple ZK payment proofs into one
- **Cross-chain proofs**: Prove Rinku payments on other chains
- **Selective disclosure**: Reveal specific fields (e.g., amount) while hiding others
- **Compliance mode**: Optional auditor keys for regulated use cases
