# rinku: Self-Provable Artifacts for Trustless Verification

[rinkuchan.com](https://rinkuchan.com)

---

**Abstract.** Today, most decentralized networks require external tooling to validate the most basic unit of their system. Sure, it's just an API call to a trusted node to validate a transaction as genuine, but what if it could be confirmed solely client-side, even _offline_? We propose a distributed network in which URLs serve as self-contained, cryptographic proofs for verification of both transaction inclusion and account state. This enables trustless confirmation without reliance on any external infrastructure or services. A rinku URL carries not just transaction data, but its complete verification path - ancestry, signatures, Merkle proofs, and checkpoint anchors. Beyond transactions, account state itself becomes provable: balances, nonces, and staking positions can be cryptographically verified against checkpoint state roots. These proof-carrying artifacts extend into a unified system - VerifiableObjects - that encompass contract receipts, account proofs, finality proofs, weight attestations, batch transaction proofs, selective state witnesses, and arbitrary custom proofs, all composable and shareable as self-provable URLs. Privacy features such as meta-transaction relaying and sender obfuscation are implemented at the contract layer rather than the protocol layer, enabling flexible privacy models without sacrificing sub-100ms fast-path confirmation. The core thesis: *the ledger proves facts. It does not expose them.*

## 1. The Problem with Modern Verification

Traditional blockchain networks require some form of extraneous software for verification, such as a light client, node, an API, etc. Other lightweight solutions such as quorum-signed receipts also exist, but they require fetching a validator set from the chain. Same story for gossip witnessing, timestamp/anchoring, even zk proofs - all of these either rely on the live network, or external infrastructure. Further, additional latency is introduced due to this secondary confirmation process. One way or another, there's an issue with trust, overhead, delays, or overall reliability. An API may be inaccessible, network conditions unreliable, or hardware restrictions surface. In the end, these gaps create risk within the user experience, and overall assurances of the transactional process.

Ultimately, *some infrastructure* is required to be trusted & relied upon to provide proofs. However in rinku's case, verification becomes self-contained. Imagine point-of-sale that is immediately confirmable on the client itself, with finality achieved within the checkpoint cadence (typically 15-30 seconds) and certainty as strong as the quorum assumption plus trust anchor.

But verification is only half the problem. Even if a transaction is provably included, the question remains: *who sent it?* On most public ledgers, every transaction's sender is permanently and publicly visible. Privacy-preserving alternatives exist - zero-knowledge proofs being the most prominent - but they introduce significant computational overhead that conflicts with real-time confirmation. Rinku addresses this at the contract layer (Section 7), enabling flexible privacy models without constraining the base protocol.

## 2. URLs as Proofs

Instead of storing data on-chain and fetching proofs from nodes, we can encode proofs directly into URLs that are returned within a transaction's receipt:

`rinku://tx/{base64url(deflate(transaction + ancestry + checkpoint))}`

A rinku URL contains:

- The transaction data (sender, recipient, amount, signature)
- Ancestry chain back to a finalized checkpoint
- Checkpoint anchor (Merkle root, validator attestations)

## 3. How It Works

### 3.1 Transaction Encoding

Transactions are encoded as compressed JSON directly into the URL:

`Transaction -> JSON -> DEFLATE -> Base64url -> URL`

A single transaction URL is roughly 600 characters. Using 5 levels of ancestry (proving the transaction chains back to a checkpoint), URLs remain under 1,500 characters which fits inside a QR code.

### 3.2 Proof Structure

A proof bundle contains (fields vary by profile - see Section 5):

```json
{
  "tx": {
    "from": "a1b2c3...",
    "to": "d4e5f6...",
    "amount": "1000000000",
    "nonce": "42",
    "sig": "..."
  },
  "fromPubKey": "BASE64_ECDSA_P256_PUBLIC_KEY",
  "hash": "sha256(tx)",
  "parents": [/* recursive proof bundles */],
  "txInclusion": {
    "merkleIndex": "7",
    "merklePath": ["sibling0", "sibling1", "..."]
  },
  "checkpoint": {
    "id": "cp_789",
    "txMerkleRoot": "...",
    "stateRoot": "...",
    "height": "1000",
    "blsAggregateSig": "...",
    "signerBitmap": "...",
    "validatorProof": { /* see Section 5.3 */ }
  }
}
```

**Key elements:**

- `fromPubKey` - The sender's full ECDSA P-256 public key, enabling signature verification. The `from` field is its fingerprint (hex(SHA-256(pubKey))[0:40])
- `txInclusion` - Merkle path proving this transaction is included in the checkpoint's `txMerkleRoot`. Required for Profile B and C.
- `checkpoint` - Contains finality data. Profile B includes checkpoint header + tx inclusion. Profile C adds `validatorProof` for fully offline verification.

Each parent itself becomes a proof bundle, creating a recursive structure that traces back to a known checkpoint.

### 3.3 Verification Algorithm

```
function verifyBundle(bundle, profile):
  // 1. Verify pubkey matches fingerprint
  fingerprint = hex(sha256(bundle.fromPubKey))[0:40]
  assert fingerprint == bundle.tx.from

  // 2. Verify transaction signature via the pubkey
  assert ecdsaVerify(bundle.fromPubKey, bundle.tx, bundle.tx.sig)

  // 3. Verify hash integrity
  assert sha256(canonicalize(bundle.tx)) == bundle.hash

  // 4. Verify all parents recursively
  for parent in bundle.parents:
    assert verifyBundle(parent, profile)

  // 5. Profile B/C: Verify tx Merkle inclusion
  if profile in ["txp", "sp"]:
    computedRoot = merkleRoot(bundle.hash, 
                              bundle.txInclusion.merklePath, 
                              bundle.txInclusion.merkleIndex)
    assert computedRoot == bundle.checkpoint.txMerkleRoot

  // 6. Profile C: Verify checkpoint finality (validator signatures)
  if profile == "sp":
    assert verifyValidatorProof(bundle.checkpoint)
    assert verifyBlsAggregate(bundle.checkpoint)

  return true
```

### 3.4 Data Availability

**Important clarification:** Self-provable URLs guarantee **verification** without infrastructure, but not **discovery**. The URL must reach the verifier through some sort of transport, such as:

- Shared URL via QR code, message, email etc
- URLs are stored in application databases
- Crawlers can follow parent references to reconstruct comprehensive history

### 3.5 The rinku Network

rinku is a DAG-based distributed network where:

* **Shared DAG structure** - Transactions reference 1-2 recent global **tip** transactions (the latest unconfirmed transactions across the network), weaving all network activity into a unified DAG
* **Nonce-based ordering** - Each account maintains the _classic_ sequential nonce to prevent replay attacks & establish per-account ordering
* **Weight-based consensus** - Conflicts are resolved by cumulative weight (based on account age & stake), ensuring Sybil resistance

**Finality via checkpoints:**

Validators periodically create checkpoints that finalize batches of transactions. A checkpoint includes:

* A Merkle root committing to all finalized transactions
* A BLS-aggregated signature from >= 67% of the validator set
* A commitment to the next validator set

Once a transaction is included in a checkpoint and confirmed, a self-provable URL is generated. The checkpoint's aggregated signature becomes the trust anchor embedded inside the proof.

## 4. Beyond Inclusion: Checkpoint-Bounded State Proofs

Transaction inclusion proofs answer: *"Did this transaction happen?"* But many use cases require proving account state: *"What was this account's balance?"* or *"Was this account staking?"*

Traditional blockchains (e.g., Ethereum) struggle with state proofs because:

1. **State is ephemeral** - Balances change every block (~12 seconds)
2. **State tries are massive** - Ethereum's state trie exceeds 100GB
3. **Proofs require live queries** - You must ask a node for current state

Rinku takes a different approach: **checkpoint-bounded state proofs**.

### 4.1 The Checkpoint-Bounded Paradigm

Instead of proving *current* state (impossible to share - it's stale before the URL loads), we prove *checkpoint-bounded* state:

> "At checkpoint N, account X had balance Y, nonce Z, and stake W."

This is an **immutable historical fact**. Checkpoint N is finalized. The state root at that checkpoint will never change. A proof generated today will verify identically in 10 years.

### 4.2 Account State Proofs

An account state proof contains:

```json
{
  "address": "96e8fccf0830f1c4...",
  "checkpoint_height": "847",
  "checkpoint_hash": "a3f2b8c9...",
  "state_root": "7d4e2a1b...",
  "balance_micro": "1170030000000",
  "balance": "11700.3",
  "nonce": "42",
  "staked_micro": "1950000000000",
  "staked": "19500.0",
  "merkle_proof": ["sibling1_hash", "sibling2_hash", "..."],
  "merkle_index": "156"
}
```

Balances are represented in two forms: `balance_micro` / `staked_micro` are canonical u64 decimal strings (1 RKU = 100,000,000 micro-RKU) used for deterministic binary hashing, while `balance` / `staked` are human-readable values. All uint64 fields are encoded as decimal strings to avoid IEEE 754 precision loss.

**Verification algorithm:**

```
function verifyAccountState(proof):
  // 1. Reconstruct leaf using binary encoding (see Appendix A.3)
  leaf = SHA256(0x01 || hex_decode(address) || uint64be(balance_micro) || uint64be(nonce) || uint64be(staked_micro))

  // 2. Walk Merkle path to root
  computed_root = walkMerklePath(leaf, proof.path, proof.position)

  // 3. Verify against checkpoint's state root
  assert computed_root == proof.state_root

  // 4. Verify checkpoint itself (BLS signatures, validator set)
  assert verifyCheckpoint(proof.checkpoint_hash, proof.state_root)

  return true
```

The URL format:

```
rinku://acct/{base64url(deflate(account_state_proof))}
```

### 4.3 What Can Be Proven

With checkpoint-bounded proofs, we can prove any fact that was true at a finalized checkpoint:

| Proof Type | What It Proves | Use Case |
|------------|----------------|----------|
| Transaction Inclusion | "Tx X is in checkpoint N" | Payment receipts, audit trails |
| Account Balance | "Address A had B tokens at checkpoint N" | Wealth verification, collateral proofs |
| Staking Position | "Address A had S tokens staked at checkpoint N" | Governance eligibility, validator status |
| Nonce State | "Address A had nonce N at checkpoint N" | Replay attack detection, ordering proofs |

### 4.4 The Freshness Tradeoff

Checkpoint-bounded proofs have an inherent tradeoff:

**Freshness vs. Provability**

- Live state is fresh but unprovable (requires trust in a node)
- Checkpoint state is provable but historical (15-30 seconds old)

For most use cases, this tradeoff is acceptable:

- Payment receipts, audit trails, governance eligibility, collateral verification
- Not suitable for real-time balance queries (use live API instead)

### 4.5 Toward Statelessness

Account state proofs move rinku closer to a stateless architecture:

**Traditional model:**
```
Verifier needs: Full state database OR trusted API
```

**Rinku model:**
```
Verifier needs: Checkpoint trust anchor (genesis or pinned) + URL
```

A verifier with only a pinned checkpoint can validate:
- Any transaction included after that checkpoint
- Any account state at any checkpoint after their trust anchor
- Without downloading the full chain
- Without querying any node
- Completely offline

This isn't full statelessness (you can't *discover* proofs without infrastructure), but it's **verification statelessness** - once you have a proof URL, you need nothing else.

## 5. VerifiableObjects: Unified Proof-Carrying Outputs

Sections 2-4 introduced specific proof types: transaction inclusion, account state, checkpoint finality. In practice, a distributed ledger generates many more kinds of provable facts - contract execution outputs, weight attestations, custom application-specific proofs. Rather than designing separate systems for each, rinku introduces **VerifiableObjects**: a unified envelope for any proof-carrying output.

### 5.1 The Problem with Fragmented Proofs

Most blockchain ecosystems treat different proof types as separate concerns:

- Transaction receipts use one format
- State proofs use another
- Oracle attestations are entirely off-chain
- Contract outputs require indexing infrastructure to retrieve

This fragmentation means each proof type needs its own verification logic, its own transport mechanism, and its own trust model. Composing proofs across types requires brittle glue code.

### 5.2 The VerifiableObject Envelope

A VerifiableObject is a tagged union where the `type` discriminator selects the variant, and type-specific fields appear at the top level alongside common fields:

```json
{
  "type": "account_proof",
  "address": "96e8fccf0830f1c4...",
  "balanceMicro": "1170030000000",
  "nonce": "42",
  "stakedMicro": "1950000000000",
  "checkpointHeight": "847",
  "checkpointHash": "a3f2b8c9...",
  "stateRoot": "7d4e2a1b...",
  "merkleProof": ["..."],
  "merkleIndex": "156",
  "chainId": "rinku-mainnet"
}
```

Every VerifiableObject, regardless of type, carries:

- A `type` discriminator identifying the variant
- A `checkpointHeight` anchoring it to a finalized state
- An optional `chainId` for cross-network disambiguation
- Type-specific proof fields at the top level (not nested under a `proof` key)

The supported types are:

| Type | What It Proves | Key Fields |
|------|----------------|------------|
| `account_proof` | Account state at checkpoint N | balance, nonce, stake, Merkle proof against state root |
| `tx_finality` | Transaction finality with validator attestation | tx hash, signature, Merkle proof, BLS aggregate, signer bitmap |
| `contract_output` | Contract execution result with state proof | StatefulReceipt (view keys, multi-proof, finality certificate) |
| `weight_proof` | Transaction weight attestation score | tx hash, aggregated weight, weight trie proof |
| `custom` | Application-defined provable fact | schema ID, payload, proof root, Merkle proof |
| `batch_proof` | Multiple transaction inclusions in a single checkpoint | shared finality context, tx hashes, Merkle multiproof |
| `state_witness` | Selective key-value state at a checkpoint | contract ID, key-value entries, sparse Merkle proofs, state root |

### 5.3 Self-Provable URLs for VerifiableObjects

Every VerifiableObject can be encoded as a self-provable URL:

```
rinku://vo/{base64url(deflate(verifiable_object))}
```

This means a contract receipt, an account balance proof, a finality certificate, and a weight attestation all share the same transport, encoding, and verification entry point. A verifier that understands VerifiableObjects can validate any proof type without knowing its specific structure in advance.

### 5.4 Proof Composability via BYOP

VerifiableObjects enable **Bring Your Own Proof (BYOP)** transactions. A contract call can include `ProofInput` arrays - VerifiableObjects from other contracts or proof sources - as verified inputs:

```json
{
  "label": "collateral_proof",
  "proof": { /* VerifiableObject: account_proof showing balance >= threshold */ },
  "expectation": {
    "expected_object_type": "account_proof",
    "min_checkpoint_height": 800,
    "expected_chain_id": "rinku-mainnet"
  }
}
```

The runtime validates each ProofInput against its expectation before contract execution begins. This allows contracts to condition their logic on externally-proven facts without requiring oracle infrastructure. A lending contract can accept a collateral proof. A governance contract can accept a staking proof. A bridge contract can accept a finality proof from another chain. In each case, the proof is self-contained and verified by the runtime, not fetched from an external service.

Contracts effectively become their own oracles.

### 5.5 BatchProof: Aggregatable Multi-Receipt Verification

When a verifier needs to confirm multiple transactions from the same checkpoint, generating and transmitting individual proofs for each transaction is wasteful. Each individual proof carries its own copy of the checkpoint finality context (BLS signature, signer bitmap, checkpoint hash) and its own independent Merkle path. For N transactions, this means N copies of the ~400-800 byte finality context and N independent Merkle paths with redundant sibling nodes.

A **BatchProof** solves this by combining multiple transaction inclusion proofs into a single VerifiableObject:

```json
{
  "type": "batch_proof",
  "finality": {
    "checkpointHeight": "1200",
    "checkpointHash": "a3f2b8c9...",
    "stateRoot": "7d4e2a1b...",
    "receiptRoot": "...",
    "blsAggregatedSig": "...",
    "blsSignerBitmap": "..."
  },
  "txHashes": ["hash1", "hash2", "hash3", "hash4"],
  "multiproof": {
    "leafHashes": ["...", "...", "...", "..."],
    "leafIndices": ["2", "5", "11", "14"],
    "helperHashes": ["...", "..."],
    "helperIndices": [["0", "3"], ["1", "1"]],
    "numLeaves": "20",
    "root": "..."
  },
  "receipts": null,
  "chainId": "rinku-mainnet"
}
```

All integer fields in `MerkleMultiProof` (`leafIndices`, `helperIndices`, `numLeaves`) are encoded as decimal strings to avoid IEEE 754 double-precision truncation at large tree sizes. Implementations must treat these as u64 values, not floating-point numbers.

**Space savings:** The checkpoint finality context is shared (1 copy instead of N). The Merkle multiproof eliminates redundant sibling nodes - when two included leaves share a common ancestor, the sibling needed by one leaf is derivable from the other. For N transactions in a tree of M total leaves, the multiproof requires at most M - N helper nodes instead of N × log2(M) individual siblings. In practice, batches of 10 transactions from a 1000-transaction checkpoint save ~60-70% compared to individual proofs.

**Multiproof algorithm and ordering rules:** The multiproof uses a deterministic bottom-up reconstruction. `leafIndices` specifies each included leaf's position in the original tree (0-indexed, left-to-right). `helperIndices` is an array of `[layer, position]` pairs, where `layer` is the tree level (0 = leaf layer, increasing upward) and `position` is the node's index within that layer. Helpers are ordered by layer ascending, then position ascending within each layer. During verification, the algorithm processes the tree layer by layer from bottom to top: for each pair of sibling positions at the current layer, if both children are known (either as included leaves or previously computed parents), the parent is computed directly via `SHA256(left || right)` with no helper consumed. If only one child is known, the next helper hash from `helperHashes` is consumed as the missing sibling, in the order specified by `helperIndices`. This continues until a single root is produced. The deterministic ordering ensures all conforming implementations reconstruct identical roots.

**Verification:** The verifier reconstructs the Merkle root from the included leaf hashes and helper nodes using the algorithm above, then checks it against the checkpoint's `txMerkleRoot`. The shared finality context (BLS aggregate signature) is verified once.

**Optional receipts:** The `receipts` field can optionally include full `StatefulReceipt` objects for each transaction, enabling the verifier to also inspect contract execution results alongside inclusion proof.

### 5.6 StateWitness: Selective State Proofs

While account state proofs (Section 4) prove an entire account's state (balance, nonce, stake), many applications need to prove specific key-value pairs from contract storage. A **StateWitness** provides selective state proofs for arbitrary contract storage keys.

#### 5.6.1 State Root Architecture

The checkpoint `stateRoot` is a composite commitment that anchors multiple sub-tries into a single verifiable root:

```
stateRoot = SHA256(accountStateRoot || contractsRoot)
```

- **`accountStateRoot`** — root of the standard Merkle tree over all account leaves (balance, nonce, stake), as described in Section 4.2.
- **`contractsRoot`** — root of a map from `contractId` to `contractStorageRoot`, where each `contractStorageRoot` is the root of a 256-level sparse Merkle trie over that contract's key-value storage.

A StateWitness proves a path through this hierarchy: first, that the contract's storage root is committed under `contractsRoot`, and second, that specific key-value entries are included (or absent) in that contract's sparse Merkle trie. When `contractId` is `null`, the witness proves account-level state directly against `accountStateRoot`.

This two-level commitment model means a verifier can validate selective contract state without downloading the full account trie or any other contract's state — each proof is self-contained against the checkpoint's `stateRoot`.

```json
{
  "type": "state_witness",
  "contractId": "sc_escrow_v1",
  "entries": [
    {
      "key": "balance:alice",
      "value": 5000000000,
      "proofKey": "a1b2c3...",
      "proofSiblings": ["sibling1", "sibling2", "..."]
    },
    {
      "key": "balance:bob",
      "value": 3000000000,
      "proofKey": "d4e5f6...",
      "proofSiblings": ["sibling1", "sibling2", "..."]
    }
  ],
  "stateRoot": "7d4e2a1b...",
  "checkpointHeight": "1200",
  "checkpointHash": "a3f2b8c9...",
  "blsAggregatedSig": "...",
  "blsSignerBitmap": "...",
  "chainId": "rinku-mainnet"
}
```

Each entry contains:
- `key` - The human-readable contract storage key
- `value` - The stored value (or `null` for non-existent keys, proving absence)
- `proofKey` - The hex-encoded hash of the key in the sparse Merkle trie
- `proofSiblings` - The sparse Merkle proof siblings proving inclusion (or exclusion) against the state root

**Use cases:**
- **Stateless dApps:** A client that has never interacted with a contract can receive a StateWitness from another party, verifying specific state values without querying any node
- **Cross-contract verification:** A contract can accept a StateWitness as a BYOP proof input, conditioning its logic on another contract's verified state
- **Audit and compliance:** Prove specific contract state entries at a historical checkpoint without exposing the entire contract state
- **Absence proofs:** When `value` is `null`, the proof demonstrates that the key does not exist in the contract's state trie at that checkpoint

**Verification:** For each entry, the verifier hashes the key to obtain `proofKey`, walks the sparse Merkle proof siblings to reconstruct the root, and checks it against the `stateRoot`. The state root itself is anchored to the checkpoint via BLS aggregate signature verification.

## 6. Eliminating Indexer Infrastructure: Stateless dApps

Traditional smart contract platforms suffer from a fundamental read problem: contracts store state on-chain, but querying that state at scale requires a separate indexing layer. On Ethereum, this manifests as The Graph - a decentralized indexing protocol that re-processes every block, extracts events, builds queryable databases, and serves GraphQL APIs. Without it, most dApps cannot function. This creates a paradox: a "decentralized" application depends on centralized query infrastructure to display a user's own data.

Rinku's stateless dApp architecture eliminates this dependency through **StatefulReceipts** and the **Proof-Carrying Contract** standard.

### 6.1 ViewKeySpec: Contracts Declare Their Own Read Interface

Every rinku contract defines a `ViewKeySpec` - a schema declaring which state values are relevant to callers. Instead of exposing generic view functions that must be queried via RPC, the contract declares upfront what state the caller will need after a mutation:

```rust
// Contract declares its read interface at deploy time
ViewKeySpec {
  keys: ["balance", "total_supply", "last_transfer_time"]
}
```

### 6.2 StatefulReceipts: Write-and-Receive

Every mutating contract call returns a `StatefulReceipt` containing:

- **View key values** - the contract's relevant state (balance, position, membership, etc.) as declared by the ViewKeySpec
- **Merkle multi-proof** - proving those values against the checkpoint state root
- **Finality certificate** - anchoring the proof to a finalized checkpoint

The client is **persistently stateless** - it never queries view functions, never polls for updates, never depends on an indexer. After every interaction, the client already holds a cryptographically verified snapshot of the state it cares about. Correctness is local.

**Traditional dApp read path:**

```
User submits tx -> Contract mutates state -> dApp queries indexer -> Indexer re-processes blocks -> Indexer builds database -> dApp displays state
```

**Rinku's write-only client model:**

```
User submits tx -> Contract mutates state -> StatefulReceipt returned -> Client holds verified state
```

### 6.3 What This Eliminates

| Traditional Stack Layer | Purpose | Rinku Equivalent |
|------------------------|---------|------------------|
| Indexer (The Graph) | Re-process blocks, build query DB | Not needed - receipts carry state |
| Subgraph deployments | Define what to index per contract | Not needed - contracts declare ViewKeySpec |
| GraphQL API layer | Serve indexed data to frontends | Not needed - clients hold receipts |
| RPC node (for reads) | Query contract view functions | Not needed - receipts are self-proving |
| Caching layer | Reduce read latency | Not needed - state is already local |

### 6.4 Cross-User State Sharing

Because StatefulReceipts are VerifiableObjects, users can share verified state with each other without infrastructure. A receipt proving "I have 500 tokens in this escrow" can be handed to a counterparty, who verifies it offline against the checkpoint state root. This enables receipt composability - contracts can accept other contracts' receipts as BYOP proof inputs, acting as their own oracles without external oracle infrastructure.

**Cold-start and cross-user scenarios:** StatefulReceipts solve the "I interacted and want my own state" case, but what about a client that has *never* interacted with a contract and needs verified state? StateWitness (Section 5.6) fills this gap. A new user joining an escrow contract can request a StateWitness for specific keys (e.g., `["balance:alice", "total_locked", "expiry"]`) and verify them against the checkpoint state root without downloading the full contract state or having any prior interaction history. This extends the "remove indexers" claim from "users can verify their own state" to "anyone can verify any selective contract state."

**The tradeoff:** Arbitrary historical queries across all accounts (e.g., "list all holders of token X") still require infrastructure. But for the vast majority of dApp use cases - "what is *my* balance, *my* position, *my* vote" - receipts cover it completely. For cross-user and cold-start scenarios, StateWitness provides selective verified state on demand. The indexer layer is not optimized; it is removed.

## 7. Privacy and Meta-Transactions: A Contract-Layer Concern

Self-provable artifacts solve the verification problem. But on a public ledger, every transaction permanently records its sender's address. Even if the proof is portable and offline-verifiable, the on-chain record reveals *who* transacted with *whom*. For a protocol whose thesis is "the ledger proves facts, it does not expose them," this is a fundamental tension.

### 7.1 Why Contract-Layer, Not Protocol-Layer

Privacy and meta-transaction relay functionality are implemented at the **contract layer** rather than embedded in the base protocol. This is a deliberate design decision:

1. **Contracts can't initiate transactions** - they are passive, executing only when called. Someone must still submit the outer transaction. This means the off-chain relay step (accepting an intent, wrapping it, submitting) is inherently an off-chain coordination problem, not a consensus problem.

2. **Flexibility over rigidity** - Different dApps have different privacy needs. A gasless onboarding flow (dApp pays gas for users) has no privacy requirement at all. A privacy-focused relay needs sender obfuscation. A DeFi protocol needs MEV protection. Encoding one model into the protocol forces all use cases into one shape.

3. **Upgradeability** - Contract-level relay logic can be upgraded, forked, and customized without protocol hard forks. Fee structures, staking requirements, privacy policies, and slashing conditions are all contract parameters.

4. **Minimal protocol surface** - The base protocol only needs to verify "the inner signer authorized this action" when a transaction includes a meta-transaction payload. Everything else - relay pools, fee markets, privacy policies, staking/slashing - lives in contract code.

### 7.2 Meta-Transaction Support

The protocol provides minimal support for meta-transactions through the `data` field of standard transactions. A meta-transaction contract verifies:

1. The inner signer's authorization (signature over the intent)
2. Nonce management (preventing replay)
3. Gas payment (the outer signer pays gas, the inner signer's balance covers the transfer)

This is analogous to ERC-2771 / ERC-4337 on Ethereum - the protocol doesn't know about relayers, but contracts can implement arbitrary relay logic.

### 7.3 Contract-Level Privacy Models

Smart contracts can implement various privacy models:

**Sponsored relay (gasless UX):** A dApp deploys a relayer contract that pays gas on behalf of its users. The sender's identity may or may not be hidden on-chain. The primary goal is UX, not privacy. Low minimum stake, simple logic.

**Privacy relay:** A contract implements sender obfuscation by accepting signed intents and executing them on behalf of users. The contract can enforce staking, slashing for deanonymization, and fee markets. Privacy guarantees depend on the contract's design and the relayer set's honesty.

**ZK privacy:** For cryptographic sender privacy where latency is acceptable, contracts can verify ZK proofs (Groth16 ZK-SNARKs). This adds seconds of proof generation latency but provides information-theoretic privacy rather than economic privacy.

#### 7.3.1 Privacy Modes Summary

To eliminate ambiguity about what is visible on-ledger in each privacy model, the following table defines the four canonical modes:

| Mode | Name | Sender on-chain | Gas payer | Privacy guarantee | Latency impact |
|------|------|-----------------|-----------|-------------------|----------------|
| 0 | Normal transaction | Public (sender address in `from` field) | Sender | None — fully transparent | None |
| 1 | Sponsored relay | Public (sender visible in `data` payload) | Relayer contract / dApp | None — gasless UX only | Minimal (contract execution) |
| 2 | Obfuscation relay | Commitment only (hash or encrypted blob in `data`; sender address NOT in plaintext on-chain) | Relayer contract | Economic — depends on relayer set honesty and contract slashing rules | Minimal (contract execution) |
| 3 | ZK mode | Cryptographically hidden (ZK proof of authorization, no sender revealed) | Relayer or sender (via shielded pool) | Information-theoretic — sender privacy guaranteed by cryptographic proof | 2-10s proof generation |

Mode 0 is the protocol default. Modes 1-3 are implemented entirely at the contract layer (Sections 7.1-7.2) and do not require protocol changes. A single user can use different modes for different transactions. The ledger proves the same facts regardless of which mode was used — only the sender's visibility changes.

### 7.4 Optional ZK Layer

The protocol supports an optional ZK privacy layer using Groth16 ZK-SNARKs for use cases where cryptographic sender privacy is required. ZK proof verification is available as a contract-level primitive.

| Approach | Proof Generation | Verification | Fast-Path Impact |
|----------|-----------------|--------------|-----------------|
| Standard transaction | N/A | Standard sig verify | No impact |
| Contract-level relay | N/A | Contract execution | Minimal overhead |
| ZK privacy (Groth16) | 2-10 seconds | ~10ms | Sender must generate proof before submission |

Users choose their privacy level per transaction based on their needs. The ledger proves the same facts regardless of which approach was used.

## 8. Proof Profiles

Different use cases require different security/size tradeoffs. Each profile includes specific fields:

### 8.1 Profile A: Authorization Only (~600 - 1,200 characters)

**What it proves:** Transaction is validly signed by the sender
**What it does NOT prove:** Finality or checkpoint inclusion
**Trust assumption:** Verifier trusts an external source confirmed finality
**Use case:** Lightweight receipts where finality is verified separately

```
rinku://tx/{payload}
```

**Included fields:** `tx`, `fromPubKey`, `hash`, `parents`
**Excluded fields:** `txInclusion`, `checkpoint`

### 8.2 Profile B: Checkpoint Inclusion (~1,500 - 3,500 characters)

**What it proves:** Transaction is Merkle-included in a checkpoint's `txMerkleRoot`
**Trust assumption:** Verifier knows and trusts the checkpoint (height, roots, signatures). See Section 10 for how verifiers obtain trusted checkpoints (pinned checkpoint distributed with app, periodic signed checkpoint feed, or genesis + checkpoint chain).
**Use case:** Standard payment receipts, audit trails

```
rinku://txp/{payload}
```

**Included fields:** `tx`, `fromPubKey`, `hash`, `parents`, `txInclusion`, `checkpoint` (header only: id, txMerkleRoot, stateRoot, height, blsAggregateSig, signerBitmap)
**Excluded fields:** `checkpoint.validatorProof`

### 8.3 Profile C: Self-Contained Finality via VerifiableObject (~2,500 - 6,000 characters)

**What it proves:** Everything in Profile B, plus the validator set commitment enabling fully offline verification
**Trust assumption:** Verifier has a trust anchor (genesis or pinned checkpoint); no live queries needed
**Use case:** Offline verification, air-gapped systems, legal evidence, cross-chain bridges

All proof types (transaction finality, account state, contract output, weight attestation, batch proofs, state witnesses) are encoded as unified `VerifiableObject` instances with a single URL scheme:

```
rinku://vo/{payload}
```

Each VerifiableObject carries optional `ProofFreshness` metadata enabling proof age verification. The unified scheme replaces the legacy `rinku://sp/` and `rinku://asp/` formats.

**Included fields:** All fields including `checkpoint.validatorProof`

**Validator Proof Format (MerkleSumTree multi-proof):**

```json
{
  "validatorProof": {
    "signerLeaves": [
      { "index": 0, "pubKey": "base64...", "weight": "1000000000" },
      { "index": 3, "pubKey": "base64...", "weight": "2500000000" }
    ],
    "auxiliaryNodes": [
      { "level": 1, "index": 2, "hash": "...", "sumWeight": "500000000" }
    ],
    "rootHash": "...",
    "totalWeight": "10000000000",
    "threshold": "6666666667"
  }
}
```

- `index` - Position of validator in the canonical sorted set
- `weight` - Stake weight as uint64 string (base units, 8 decimals)
- `level` - Tree level (0 = leaves)
- `sumWeight` - Cumulative weight for sum-tree verification
- `threshold` - Minimum weight required for quorum (>= 2/3 of totalWeight)

**Verification requirement:** Verifiers MUST reconstruct `(rootHash, totalWeight)` from `signerLeaves` + `auxiliaryNodes` + EMPTY_NODE padding using the MerkleSumTree algorithm and reject if results do not match the embedded values. Treat `totalWeight` as a claim that must be verified, not trusted input.

### 8.4 Profile D: Account State (~800 - 1,800 characters)

**What it proves:** Account balance, nonce, and staking position at a specific checkpoint
**Trust assumption:** Verifier trusts the checkpoint state root (or has full checkpoint proof via Profile C)
**Use case:** Balance verification, governance eligibility, collateral proofs, airdrop qualification

```
rinku://acct/{payload}
```

**Included fields:** `address`, `checkpoint_height`, `checkpoint_hash`, `state_root`, `account_state`, `merkle_proof`

Account state proofs are compact because they contain only:
- Account address and state (balance, nonce, staked)
- Merkle path to state root (~log2(accounts) hashes)
- Checkpoint reference (hash, height, state root)

For a network with 1 million accounts, the Merkle path is ~20 hashes (640 bytes), making the total proof well under 1KB compressed.

### 8.5 Profile E: VerifiableObject (~variable)

**What it proves:** Any proof-carrying output (contract receipt, weight attestation, custom proof)
**Trust assumption:** Depends on the enclosed proof type
**Use case:** Contract state sharing, cross-contract composition, portable attestations

```
rinku://vo/{payload}
```

**Included fields:** `type`, type-specific proof data, `checkpoint_height`, `chain_id`

### 8.6 Profile F: BatchProof (~variable, significant savings at scale)

**What it proves:** Multiple transaction inclusions within a single checkpoint, using a shared finality context and Merkle multiproof
**Trust assumption:** Same as Profile B/C (verifier trusts checkpoint or has full validator proof)
**Use case:** Batch payment verification, audit trails covering multiple transactions, bulk settlement proofs

```
rinku://vo/{payload}  (type: "batch_proof")
```

**Included fields:** `finality` (shared checkpoint context), `txHashes`, `multiproof` (Merkle multiproof), optional `receipts`, `chainId`

**Size comparison (10 transactions from a 1000-tx checkpoint):**

| Approach | Finality Context | Merkle Data | Total Estimate |
|----------|-----------------|-------------|----------------|
| 10 × Individual Profile B | 10 × ~600 bytes | 10 × ~320 bytes (10 siblings each) | ~9,200 bytes |
| 1 × BatchProof (Profile F) | 1 × ~600 bytes | ~640 bytes (multiproof: ~20 helpers) | ~2,840 bytes |
| **Savings** | | | **~69%** |

The savings increase with batch size and decrease as the ratio of included leaves to total leaves approaches 1. For a full checkpoint (all transactions included), the multiproof requires zero helper nodes and only the shared finality context.

### 8.7 Proof Freshness and Expiry

Checkpoint-bounded proofs are immutable historical facts, but their usefulness degrades over time. A proof that an account held 10,000 RKU at checkpoint 500 is less meaningful when the chain is at checkpoint 50,000. **Proof freshness** provides metadata that enables verifiers to evaluate how recent a proof is and reject stale proofs that no longer reflect current state.

#### 8.7.1 Purpose

Without freshness controls, proof-carrying artifacts are vulnerable to replay attacks using outdated state. Consider a merchant accepting an account balance proof as collateral evidence: an adversary could present a proof from checkpoint 100 showing a large balance, even though the funds were spent by checkpoint 200. Proof freshness enables the merchant to specify *"I only accept proofs generated within the last 5 checkpoints,"* preventing stale proof replay.

#### 8.7.2 ProofFreshness Metadata

Every VerifiableObject carries an optional `freshness` field containing:

| Field | Type | Description |
|-------|------|-------------|
| `generatedAtCheckpoint` | u64 | The checkpoint height at which the proof was generated |
| `generatedAtTimestamp` | u64 | Unix timestamp (milliseconds) when the proof was generated |
| `chainTipAtGeneration` | u64 | The chain tip checkpoint height at proof generation time |
| `maxAgeCheckpoints` | u64? | Optional maximum age (in checkpoints) before the proof is considered expired |

The `freshness` field is optional to maintain backward compatibility with proofs generated before the freshness system was introduced.

#### 8.7.3 Age Calculation

The age of a proof is defined as the distance in checkpoints between when the proof was generated and the current chain tip:

```
age = current_checkpoint - generated_at_checkpoint
```

This uses saturating subtraction (never goes below zero). The `current_checkpoint` is the verifier's view of the chain tip, obtainable via the `GET /api/chain/tip` endpoint which returns:

```json
{
  "checkpointHeight": "1250",
  "checkpointHash": "a3f2b8c9...",
  "checkpointTimestamp": "1709123456000",
  "stateRoot": "7d4e2a1b..."
}
```

#### 8.7.4 Freshness Validation

When a `ProofExpectation` includes `maxAgeCheckpoints` and `currentCheckpointHeight`, the runtime enforces freshness during `ProofInput` validation:

1. If the proof carries a `freshness` field, compute `age = currentCheckpointHeight - freshness.generatedAtCheckpoint`. If `age > maxAgeCheckpoints`, reject with `ProofTooOld`.
2. If the proof does not carry freshness metadata (legacy proofs), fall back to `age = currentCheckpointHeight - proof.checkpointHeight`. If `age > maxAgeCheckpoints`, reject.

This two-tier approach ensures that legacy proofs without freshness metadata are still subject to age limits based on their checkpoint height, while proofs with explicit freshness metadata benefit from more precise age tracking.

#### 8.7.5 Example: Merchant Freshness Requirement

A lending contract requires collateral proofs no older than 5 checkpoints:

```json
{
  "label": "collateral_proof",
  "proof": {
    "type": "account_proof",
    "address": "96e8fccf0830f1c4...",
    "balanceMicro": "5000000000000",
    "freshness": {
      "generatedAtCheckpoint": "1248",
      "generatedAtTimestamp": "1709123400000",
      "chainTipAtGeneration": "1248",
      "maxAgeCheckpoints": "10"
    }
  },
  "expectation": {
    "expected_object_type": "account_proof",
    "max_age_checkpoints": 5,
    "current_checkpoint_height": 1250
  }
}
```

In this case, the proof was generated at checkpoint 1248 and the chain is at 1250. Age = 2, which is within the 5-checkpoint limit. The proof is accepted.

If the same proof were presented when the chain reaches checkpoint 1260, age = 12, exceeding the limit. The runtime rejects with: `"proof too old: max age 5 checkpoints, actual age 12 (proof at 1248, chain at 1260)"`.

## 9. Size Analysis

Real-world measurements using high-entropy data (e.g signatures):

| Profile | Content | URL Length |
|---------|---------|------------|
| A (tx) - Single | Auth only, no Merkle | ~600 chars |
| A (tx) - 5-depth | Auth only, ancestry | ~1,400 chars |
| B (txp) - Single | + Merkle inclusion | ~900 chars |
| B (txp) - 5-depth | + Merkle inclusion | ~2,100 chars |
| C (sp) - Single | + Validator proof | ~1,800 chars |
| C (sp) - 5-depth | + Validator proof | ~3,400 chars |
| D (acct) | Account state | ~800-1,200 chars |

**Note:** Merkle paths add ~20-40 bytes per tree level (log2 of transactions). Validator proofs add ~400-800 bytes depending on signer count.

### Platform Compatibility

| Platform | Limit | Profile A/D | Profile B | Profile C |
|----------|-------|-------------|-----------|-----------|
| QR Code (L) | 2,953 bytes | 5-depth | 3-depth | 1-depth |
| QR Code (H) | 1,273 bytes | Single | Single | - |
| Browser URL | 65KB+ | Unlimited | Unlimited | Unlimited |

**Note**: QR capacity depends on encoding mode; base64url typically uses byte mode. For maximum QR density, consider base45 encoding for compact receipts.

## 10. Trust Bootstrapping

A fresh verifier needs a trust anchor:

1. **Genesis trust** - Know the genesis validator set
2. **Checkpoint chain** - Each checkpoint commits to the next validator set
3. **Pinned checkpoint** - Trust a recent checkpoint from an out-of-band source

This mirrors TLS certificate chains - the proof is self-contained, but root trust must be established externally. Once bootstrapped, all subsequent proofs verify offline.

## 11. Why This Matters

### 11.1 Infrastructure Independence

Traditional flow (simplified):

```
User -> Trust Node -> Query Proof -> Verify with Node
```

rinku flow:

```
User -> Receive URL -> Verify Locally
```

### 11.2 Portable Proofs

A rinku URL can be:

- Printed as a QR code on a receipt
- Sent via SMS, email, or messaging app
- Embedded in a PDF or document
- Stored offline indefinitely

### 11.3 Offline-First

Verification works completely offline. This enables:

- PoS in areas with poor or no connectivity
- Air-gapped security systems
- Archival verification that withstands over large periods of time
- Cross-border payments without infrastructure

### 11.4 Privacy as a Contract-Layer Concern

Privacy is not an afterthought bolted onto a transparent chain. By placing meta-transaction relay logic at the contract layer, rinku enables:

- Flexible privacy models: dApps choose their own privacy/UX tradeoff
- Sponsored transactions for seamless user onboarding (gasless UX)
- Contract-enforced sender obfuscation with custom staking/slashing
- Optional ZK privacy for cryptographic anonymity where latency is acceptable
- Layered privacy (public / contract-relayed / ZK) selectable per transaction
- Compatibility with sub-100ms fast-path confirmation (relay logic adds no consensus overhead)

## 12. Cryptographic Primitives

The proof system uses standard, well-audited cryptography:

| Component | Algorithm | Purpose |
|-----------|-----------|---------|
| Transaction signatures | ECDSA P-256 | Sender authorization |
| Hash function | SHA-256 | Transaction identity, Merkle trees |
| Validator signatures | BLS12-381 | Aggregated checkpoint attestation |
| Compression | DEFLATE | URL size reduction |
| Encoding | Base64url | URL-safe representation |
| ZK proofs (optional) | Groth16 / BN254 | Privacy-preserving transactions |

ECDSA verification can use the Web Crypto API; BLS verification can use a WASM library (e.g blst)

## 13. Limitations

**Size constraints:** Complex proofs (30+ transactions) exceed QR capacity and require standard URLs

**Bootstrap requirement:** First-time verifiers need a trust anchor (genesis or pinned checkpoint)

**URL mutability:** URLs can be shared but not modified. Proof updates require re-generated URLs

**Compression variability:** Actual compression ratios depend on transaction content. High entropy data compresses less

**Privacy model:** Privacy features (meta-transaction relaying, sender obfuscation) are implemented at the contract layer, not the base protocol. Privacy guarantees depend on the specific relay contract's design. Full cryptographic anonymity requires the optional ZK layer.

## 14. Conclusion

The rinku network demonstrates that a distributed ledger can be simultaneously self-provable, composable, and extensibly private. By encoding transaction data, ancestry chains, Merkle paths, and checkpoint anchors directly into URLs, we eliminate infrastructure dependencies for verification. By unifying all proof-carrying outputs into VerifiableObjects, we make proofs composable across contracts and applications. By placing privacy and meta-transaction relay logic at the contract layer rather than the protocol layer, we enable flexible privacy models without sacrificing real-time confirmation.

The core innovations are:

1. **Self-provable URLs** - Transaction and account state proofs encoded as portable, offline-verifiable URLs
2. **VerifiableObjects** - A unified envelope for any proof-carrying output, enabling cross-contract composition via BYOP
3. **Aggregatable proofs** - BatchProofs combine multiple transaction inclusions with shared checkpoint context and Merkle multiproofs, reducing proof size by ~60-70% at scale. StateWitnesses provide selective key-value state proofs for contract storage, enabling stateless verification of arbitrary contract state
4. **Stateless dApps** - StatefulReceipts eliminate indexer infrastructure (The Graph, subgraphs, GraphQL) by returning verified state directly to clients
5. **Contract-layer privacy** - Meta-transaction relaying, sender obfuscation, and gasless UX are implemented as smart contracts, enabling dApps to choose their own privacy/performance tradeoff without protocol changes

By embracing checkpoint-bounded proofs rather than fighting the impossible battle of proving live state, rinku achieves practical self-provability. By placing privacy at the contract layer, rinku enables diverse privacy models - from simple gasless onboarding to full ZK anonymity - without constraining the base protocol.

The write-only client model - where clients submit transactions and receive self-proving receipts - means dApps no longer require read infrastructure. Smart contracts handle the relay coordination that enables gasless transactions and sender privacy. The ledger proves facts. It does not expose them.

Ultimately, *the link is the proof*.

---

## Appendix A: Encoding Specification

### A.1 URL Format

```
rinku://{profile}/{base64url(deflate(json))}
```

Profiles:

- `tx` - Authorization only (Profile A)
- `txp` - Checkpoint inclusion (Profile B)
- `sp` - Self-contained finality proof (Profile C)
- `acct` - Account state proof (Profile D)
- `vo` - VerifiableObject (Profile E, also used for Profile F BatchProof and StateWitness)

### A.2 Transaction Schema

```typescript
interface Transaction {
  from: string;      // 40-char hex fingerprint
  to: string;        // 40-char hex fingerprint
  amount: string;    // uint64 as decimal string, base units (8 decimals)
  fee: string;       // uint64 as decimal string, base units
  nonce: string;     // uint64 as decimal string, sender sequence number
  parents: string[]; // 0-2 parent references (DAG tips)
  ts: string;        // uint64 as decimal string, Unix timestamp (ms)
  sig: string;       // Base64 ECDSA P-256 signature
  kind?: string;     // Transaction type: "transfer" | "stake" | "contract" | ...
  data?: string;     // Opaque data field (contract call, memo, etc.)
  memo?: string;     // Human-readable memo
}
```

### A.3 Account State Proof Schema

```typescript
interface AccountStateProof {
  type: "account_proof";       // VerifiableObject type tag
  address: string;             // 40-char hex fingerprint
  balance_micro: string;       // uint64 decimal string (1 RKU = 100,000,000 micro-RKU)
  balance: string;             // human-readable decimal (e.g., "117.003"), NOT used in hashing
  nonce: string;               // uint64 decimal string
  staked_micro: string;        // uint64 decimal string
  staked: string;              // human-readable decimal, NOT used in hashing
  checkpoint_height: string;   // uint64 decimal string
  checkpoint_hash: string;
  state_root: string;
  merkle_proof: string[];      // sibling hashes along Merkle path (hex)
  merkle_index: string;        // uint64 decimal string, leaf position in tree
  bls_aggregated_sig?: string;
  bls_signer_bitmap?: string;
  chain_id?: string;
}
```

**Leaf hash computation (binary encoding):**

Account leaf hashing uses fixed-width binary encoding to eliminate cross-language string formatting ambiguities:

```
DOMAIN_PREFIX  = 0x01                    // 1 byte: account leaf domain separator
address_bytes  = hex_decode(address)     // 20 bytes: raw address
balance_be     = uint64_big_endian(balance_micro)  // 8 bytes
nonce_be       = uint64_big_endian(nonce)           // 8 bytes
staked_be      = uint64_big_endian(staked_micro)    // 8 bytes

leaf_data = DOMAIN_PREFIX || address_bytes || balance_be || nonce_be || staked_be
leaf_hash = hex(SHA-256(leaf_data))
// Total: 45 bytes, always exactly 45 bytes regardless of values
```

This binary encoding ensures:
- No string formatting ambiguities (leading zeros, decimal points, locale differences)
- Fixed-width fields prevent concatenation collisions
- Domain prefix prevents cross-type hash collisions (e.g., account leaf vs. internal Merkle node)
- Deterministic across all languages (big-endian uint64 is unambiguous)

The `balance` and `staked` string fields are provided for human readability but are NOT used in proof computation. Only `balance_micro`, `nonce`, and `staked_micro` participate in hashing.

### A.4 VerifiableObject Schema

VerifiableObjects are serialized as tagged unions using `type` as the discriminator. All variant-specific fields appear at the top level (not nested under a `proof` key). Fields use camelCase serialization. All uint64 fields are encoded as decimal strings.

```typescript
type VerifiableObject =
  | { type: "contract_output"; receipt: StatefulReceipt;
      freshness?: ProofFreshness }
  | { type: "account_proof"; address: string; balanceMicro: string; nonce: string;
      stakedMicro: string; checkpointHeight: string; checkpointHash: string;
      stateRoot: string; merkleProof: string[]; merkleIndex: string;
      blsAggregatedSig?: string; blsSignerBitmap?: string; chainId?: string;
      freshness?: ProofFreshness }
  | { type: "tx_finality"; txHash: string; txSignature: string;
      checkpointHeight: string; merkleProof: MerkleProof;
      aggregatedSignature: string; signerBitmap: number[];
      validatorRoot: string; chainId?: string;
      freshness?: ProofFreshness }
  | { type: "weight_proof"; txHash: string; aggregatedWeight: AggregatedWeight;
      checkpointHeight: string; checkpointHash: string;
      weightTrieRoot: string; merkleProof: string[]; merkleIndex: string;
      blsAggregatedSig?: string; chainId?: string;
      freshness?: ProofFreshness }
  | { type: "custom"; schemaId: string; payload: number[];
      proofRoot: string; merkleProof: string[];
      checkpointHeight: string; chainId?: string;
      freshness?: ProofFreshness }
  | { type: "batch_proof"; finality: CheckpointFinality;
      txHashes: string[]; multiproof: MerkleMultiProof;
      receipts?: StatefulReceipt[]; chainId?: string;
      freshness?: ProofFreshness }
  | { type: "state_witness"; contractId?: string;
      entries: StateWitnessEntry[]; stateRoot: string;
      checkpointHeight: string; checkpointHash: string;
      blsAggregatedSig?: string; blsSignerBitmap?: string;
      chainId?: string;
      freshness?: ProofFreshness };

interface ProofFreshness {
  generatedAtCheckpoint: string;   // u64 decimal string
  generatedAtTimestamp: string;    // u64 decimal string (Unix ms)
  chainTipAtGeneration: string;    // u64 decimal string
  maxAgeCheckpoints?: string;      // u64 decimal string, optional
}

interface MerkleMultiProof {
  leafHashes: string[];
  leafIndices: string[];    // u64 decimal strings (not IEEE doubles)
  helperHashes: string[];
  helperIndices: [string, string][];  // [layer, position] as u64 decimal strings
  numLeaves: string;        // u64 decimal string
  root: string;
}

interface StateWitnessEntry {
  key: string;
  value: any | null;
  proofKey: string;
  proofSiblings: string[];
}
```

### A.5 Canonical JSON

Transaction fields are serialized in deterministic order for consistent hashing:
`from, to, amount, fee, nonce, parents, ts, sig`. See Section A.6 for the canonical JSON specification.

**Signature payload:** The signed payload is the canonical JSON of the transaction **excluding** the `sig` field. The signature is computed over this payload, then appended to the transaction as the `sig` field. This avoids circular dependency where the signature would need to sign itself.

### A.6 Canonical JSON Specification

All structures that participate in hashing (transactions, proof bundles) use a strict canonical JSON encoding:

1. **No whitespace** - No spaces, tabs, or newlines between tokens
2. **Deterministic key ordering** - Object keys are sorted lexicographically (Unicode code point order)
3. **Numeric encoding** - All uint64 values are encoded as decimal strings (e.g., `"42"` not `42`). This avoids IEEE 754 precision loss for values > 2^53 and eliminates cross-language differences in float stringification
4. **String encoding** - UTF-8, with JSON-mandated escapes for control characters, `"`, and `\`
5. **Boolean/null encoding** - `true`, `false`, `null` (lowercase, no quotes)
6. **Array encoding** - Elements in original order, no trailing commas
7. **Optional field omission** - Fields with `null`, empty string, or zero value are omitted entirely (not included as `"field": null`)

**Example (RelayIntent canonical fields):**
```
Input:  { from: "abc", amount: "3000000000", nonce: "42", to: "def", expiryMs: "1708646400000", maxGasPrice: "100000" }
Output: {"amount":"3000000000","expiryMs":"1708646400000","from":"abc","maxGasPrice":"100000","nonce":"42","to":"def"}
```

Implementations MUST NOT rely on `JSON.stringify()` for canonical encoding, as key ordering and numeric formatting are not guaranteed across languages and runtimes. Use an explicit canonicalizer that sorts keys and enforces string encoding for uint64 values.

---

## Appendix B: Reference Implementation

### B.1 Transaction Proof Verifier

```javascript
// Entry point: decode URL and verify
async function verifyProofUrl(url) {
  const [, profile, payload] = url.match(/rinku:\/\/(\w+)\/(.+)/);
  const json = inflate(base64urlDecode(payload));
  const bundle = JSON.parse(json);
  return verifyBundle(bundle, profile);
}

// Core verification logic (recursive for parents)
async function verifyBundle(bundle, profile) {
  // 1. Verify pubkey matches fingerprint
  const pubKeyBytes = base64Decode(bundle.fromPubKey);
  const pubKeyHash = await sha256Hex(pubKeyBytes);  // Returns hex string
  const fingerprint = pubKeyHash.slice(0, 40);      // First 40 hex chars
  if (fingerprint !== bundle.tx.from) {
    throw new Error('Public key does not match sender fingerprint');
  }

  // 2. Verify transaction signature using bundled pubkey
  const txBytes = canonicalize(bundle.tx);
  const pubKey = await crypto.subtle.importKey(
    'spki', pubKeyBytes,
    { name: 'ECDSA', namedCurve: 'P-256' },
    false, ['verify']
  );
  const valid = await crypto.subtle.verify(
    { name: 'ECDSA', hash: 'SHA-256' },
    pubKey,
    base64Decode(bundle.tx.sig),
    txBytes
  );
  if (!valid) throw new Error('Invalid signature');

  // 3. Verify hash integrity
  const hash = await sha256Hex(txBytes);
  if (hash !== bundle.hash) throw new Error('Hash mismatch');

  // 4. Verify parents recursively (no re-encoding needed)
  for (const parent of bundle.parents) {
    await verifyBundle(parent, profile);
  }

  // 5. Profile B/C: Verify tx Merkle inclusion in checkpoint
  if ((profile === 'txp' || profile === 'sp') && bundle.txInclusion) {
    const computedRoot = await computeMerkleRoot(
      bundle.hash,
      bundle.txInclusion.merklePath,
      bundle.txInclusion.merkleIndex
    );
    if (computedRoot !== bundle.checkpoint.txMerkleRoot) {
      throw new Error('Transaction not included in checkpoint');
    }
  }

  // 6. Profile C: Verify checkpoint finality (BLS + validator proof)
  if (profile === 'sp' && bundle.checkpoint?.validatorProof) {
    await verifyValidatorProof(bundle.checkpoint);
    await verifyBlsAggregate(bundle.checkpoint);
  }

  return true;
}

// Merkle inclusion proof verification (async - sha256Hex returns a Promise)
// Note: leafHash, path[i] are all 64-char hex strings representing 32-byte SHA-256 hashes
async function computeMerkleRoot(leafHash, path, index) {
  let current = leafHash;
  let pos = BigInt(index);  // BigInt for safe uint64 index arithmetic
  for (const sibling of path) {
    const leftBytes = pos % 2n === 0n ? hexToBytes(current) : hexToBytes(sibling);
    const rightBytes = pos % 2n === 0n ? hexToBytes(sibling) : hexToBytes(current);
    const combined = concatBytes(leftBytes, rightBytes);  // 64 bytes total
    current = await sha256Hex(combined);  // Returns 64-char hex string
    pos = pos / 2n;  // BigInt integer division
  }
  return current;
}

// Helper: concatenate two Uint8Arrays
function concatBytes(a, b) {
  const result = new Uint8Array(a.length + b.length);
  result.set(a, 0);
  result.set(b, a.length);
  return result;
}
```

### B.2 Account State Proof Verifier

```javascript
// Binary leaf hash: DOMAIN_PREFIX(1) || address(20) || balance_be(8) || nonce_be(8) || staked_be(8)
function buildAccountLeaf(address, balanceMicro, nonce, stakedMicro) {
  const buf = new Uint8Array(45);  // 1 + 20 + 8 + 8 + 8
  buf[0] = 0x01;  // domain prefix: account leaf
  buf.set(hexToBytes(address), 1);  // 20 bytes from 40-char hex
  const view = new DataView(buf.buffer);
  view.setBigUint64(21, BigInt(balanceMicro));  // big-endian
  view.setBigUint64(29, BigInt(nonce));
  view.setBigUint64(37, BigInt(stakedMicro));
  return buf;
}

async function verifyAccountStateProof(url) {
  // 1. Parse and decode url
  const [, profile, payload] = url.match(/rinku:\/\/(\w+)\/(.+)/);
  if (profile !== 'acct') throw new Error('Not an account proof');
  
  const json = inflate(base64urlDecode(payload));
  const proof = JSON.parse(json);

  // 2. Reconstruct leaf hash from binary encoding
  const leafData = buildAccountLeaf(
    proof.address, proof.balance_micro, proof.nonce, proof.staked_micro
  );
  const computedLeaf = await sha256Hex(leafData);

  // 3. Walk Merkle path from leaf to root using BigInt for indices
  const computedRoot = await computeMerkleRoot(
    computedLeaf, proof.merkle_proof, proof.merkle_index
  );

  // 4. Verify computed root matches checkpoint state root
  if (computedRoot !== proof.state_root) {
    throw new Error('Merkle root mismatch');
  }

  // 5. Return verified state (checkpoint trust assumed or verified separately)
  return {
    valid: true,
    address: proof.address,
    balance_micro: proof.balance_micro,
    balance: proof.balance,
    nonce: proof.nonce,
    staked_micro: proof.staked_micro,
    staked: proof.staked,
    checkpoint_height: proof.checkpoint_height
  };
}
```

---

## References

1. D. Boneh, M. Drijvers, and G. Neven. "Compact Multi-Signatures for Smaller Blockchains." ASIACRYPT 2018. https://crypto.stanford.edu/~dabo/pubs/papers/BLSmultisig.html
2. D. Boneh, B. Lynn, and H. Shacham. "Short Signatures from the Weil Pairing." Journal of Cryptology, 2004.
3. R. Merkle. "A Digital Signature Based on a Conventional Encryption Function." CRYPTO 1987.
4. L. Reyzin and S. Yakoubov. "Efficient Asynchronous Accumulators for Distributed PKI." SCN 2016. (Sparse Merkle Trees)
5. B. Danezis et al. "Mysticeti: Reaching the Limits of Latency with Uncertified DAGs." 2024.
6. D. Chaum. "Untraceable Electronic Mail, Return Addresses, and Digital Pseudonyms." Communications of the ACM, 1981.

---
