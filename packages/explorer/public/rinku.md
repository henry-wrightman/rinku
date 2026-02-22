# rinku: Self-Verifiable URLs for Trustless Verification

[rinkuchan.com](https://rinkuchan.com)

---

**Abstract.** Today, most decentralized networks require external tooling to validate the most basic unit of their system. Sure, it's just an API call to a trusted node to validate a transaction as genuine, but what if it could be confirmed solely client-side, even _offline_? We propose a distributed network in which URLs serve as self-contained, cryptographic proofs for verification of both transaction inclusion and account state. This enables trustless confirmation without reliance on any external infrastructure or services. A rinku URL carries not just transaction data, but its complete verification path - ancestry, signatures, Merkle proofs, and checkpoint anchors. Beyond transactions, account state itself becomes provable: balances, nonces, and staking positions can be cryptographically verified against checkpoint state roots. Ultimately, *the link itself is the proof*. This paper focuses on the URL-native proof system that the rinku network leverages, and the paradigm of checkpoint-bounded self-provability.

## 1. The Problem with Modern Verification

Traditional blockchain networks requires some form extraneous software for verification, such as a light client, node, an API, etc. Other lightweight solutions such as quorum-signed receipts also exist, but they require fetching a validator set from the chain. Same story for gossip witnessing, timestamp/anchoring, even zk proofs - all of these either rely on the live network, or external infrasture. Further, additional latency is introduced due to this secondary confirmation process. One way or another, there's an issue with trust, overhead, delays, or overall reliability. An API may be inaccessible, network conditions unreliable, or hardware restrictions surface. In the end, these gaps create risk within the user experience, and overall assurances of the transactional process.

Ultimatley, *some infrastructure* is required to be trusted & relied upon to provide proofs. However in rinku's case, verification becomes self-contained. Imagine point-of-sale that is immediately confirmable on the client itself, with finality achieved within the checkpoint cadence (typically 15-30 seconds) and certainty as strong as the quorum assumption plus trust anchor.

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
  "account_state": {
    "balance": "11700300000",
    "nonce": "42",
    "staked": "19500000000"
  },
  "merkle_proof": {
    "leaf_hash": "...",
    "path": ["sibling1", "sibling2", ...],
    "position": "156"
  }
}
```

**Verification algorithm:**

```
function verifyAccountState(proof):
  // 1. Reconstruct leaf from account state
  leaf = hash(address + balance + nonce + staked)

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

- ✅ **Qualification proofs**: "Prove you held ≥100 RKU at snapshot for airdrop eligibility"
- ✅ **Audit trails**: "Prove account state at time of disputed transaction"
- ✅ **Governance**: "Prove staking position for voting weight"
- ✅ **Collateral verification**: "Prove locked funds at checkpoint N"
- ⚠️ **Real-time balance**: Not suitable (use live API query instead)

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

### 4.6 Eliminating Indexer Infrastructure

Traditional smart contract platforms suffer from a fundamental read problem: contracts store state on-chain, but querying that state at scale requires a separate indexing layer. On Ethereum, this manifests as The Graph — a decentralized indexing protocol that re-processes every block, extracts events, builds queryable databases, and serves GraphQL APIs. Without it, most dApps cannot function. This creates a paradox: a "decentralized" application depends on centralized query infrastructure to display a user's own data.

Rinku's stateless dApp architecture eliminates this dependency entirely through **StatefulReceipts**.

**The traditional dApp read path:**

```
User submits tx -> Contract mutates state -> dApp queries indexer -> Indexer re-processes blocks -> Indexer builds database -> dApp displays state
```

**Rinku's write-only client model:**

```
User submits tx -> Contract mutates state -> StatefulReceipt returned -> Client holds verified state
```

Every mutating contract call in rinku returns a `StatefulReceipt` containing:

- **View key values** — the contract's relevant state (balance, position, membership, etc.) as declared by the contract's `ViewKeySpec`
- **Merkle multi-proof** — proving those values against the checkpoint state root
- **Finality certificate** — anchoring the proof to a finalized checkpoint

The client is **persistently stateless** — it never queries view functions, never polls for updates, never depends on an indexer. After every interaction, the client already holds a cryptographically verified snapshot of the state it cares about. Correctness is local.

**What this eliminates:**

| Traditional Stack Layer | Purpose | Rinku Equivalent |
|------------------------|---------|------------------|
| Indexer (The Graph) | Re-process blocks, build query DB | Not needed — receipts carry state |
| Subgraph deployments | Define what to index per contract | Not needed — contracts declare `ViewKeySpec` |
| GraphQL API layer | Serve indexed data to frontends | Not needed — clients hold receipts |
| RPC node (for reads) | Query contract view functions | Not needed — receipts are self-proving |
| Caching layer | Reduce read latency | Not needed — state is already local |

**Cross-user state sharing via receipts:**

Because StatefulReceipts are self-proving, users can share verified state with each other without infrastructure. A receipt proving "I have 500 tokens in this escrow" can be handed to a counterparty, who verifies it offline against the checkpoint state root. This enables **receipt composability** — contracts can accept other contracts' receipts as proof inputs (BYOP), acting as their own oracles without external oracle infrastructure.

**The tradeoff:** Clients only have state they have interacted with or been given receipts for. Arbitrary historical queries across all accounts (e.g., "list all holders of token X") still require infrastructure. But for the vast majority of dApp use cases — "what is *my* balance, *my* position, *my* vote" — receipts cover it completely. The indexer layer is not optimized; it is removed.

## 5. Proof Profiles

Different use cases require different security/size tradeoffs. Each profile includes specific fields:

### 5.1 Profile A: Authorization Only (~600 - 1,200 characters)

**What it proves:** Transaction is validly signed by the sender
**What it does NOT prove:** Finality or checkpoint inclusion
**Trust assumption:** Verifier trusts an external source confirmed finality
**Use case:** Lightweight receipts where finality is verified separately

```
rinku://tx/{payload}
```

**Included fields:** `tx`, `fromPubKey`, `hash`, `parents`
**Excluded fields:** `txInclusion`, `checkpoint`

### 5.2 Profile B: Checkpoint Inclusion (~1,500 - 3,500 characters)

**What it proves:** Transaction is Merkle-included in a checkpoint's `txMerkleRoot`
**Trust assumption:** Verifier knows and trusts the checkpoint (height, roots, signatures). See Section 7 for how verifiers obtain trusted checkpoints (pinned checkpoint distributed with app, periodic signed checkpoint feed, or genesis + checkpoint chain).
**Use case:** Standard payment receipts, audit trails

```
rinku://txp/{payload}
```

**Included fields:** `tx`, `fromPubKey`, `hash`, `parents`, `txInclusion`, `checkpoint` (header only: id, txMerkleRoot, stateRoot, height, blsAggregateSig, signerBitmap)
**Excluded fields:** `checkpoint.validatorProof`

### 5.3 Profile C: Self-Contained Finality (~2,500 - 6,000 characters)

**What it proves:** Everything in Profile B, plus the validator set commitment enabling fully offline verification
**Trust assumption:** Verifier has a trust anchor (genesis or pinned checkpoint); no live queries needed
**Use case:** Offline verification, air-gapped systems, legal evidence, cross-chain bridges

```
rinku://sp/{payload}
```

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

### 5.4 Profile D: Account State (~800 - 1,800 characters)

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

## 6. Size Analysis

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

## 7. Trust Bootstrapping

A fresh verifier needs a trust anchor:

1. **Genesis trust** - Know the genesis validator set
2. **Checkpoint chain** - Each checkpoint commits to the next validator set
3. **Pinned checkpoint** - Trust a recent checkpoint from an out-of-band source

This mirrors TLS certificate chains - the proof is self-contained, but root trust must be established externally. Once bootstrapped, all subsequent proofs verify offline.

## 8. Why This Matters

### 8.1 Infrastructure Independence

Traditional flow (simplified):

```
User -> Trust Node -> Query Proof -> Verify with Node
```

rinku flow:

```
User -> Receive URL -> Verify Locally
```



### 8.2 Portable Proofs

A rinku URL can be:

- Printed as a QR code on a receipt
- Sent via SMS, email, or messaging app
- Embedded in a PDF or document
- Stored offline indefinitely

### 8.3 Offline-First

Verification works completely offline. This enables:

- PoS in areas with poor or no connectivity
- Air-gapped security systems
- Archival verification that withstands over large periods of time
- Cross-border payments without infrastructure

## 9. Cryptographic Primitives

The proof system uses standard, well-audited cryptography:

| Component | Algorithm | Purpose |
|-----------|-----------|---------|
| Transaction signatures | ECDSA P-256 | Sender authorization |
| Hash function | SHA-256 | Transaction identity, Merkle trees |
| Validator signatures | BLS12-381 | Aggregated checkpoint attestation |
| Compression | DEFLATE | URL size reduction |
| Encoding | Base64url | URL-safe representation |

ECDSA verification can use the Web Crypto API; BLS verification can use a WASM library (e.g blst)

## 10. Limitations

**Size constraints:** Complex proofs (30+ transactions) exceed QR capacity and require standard URLs

**Bootstrap requirement:** First-time verifiers need a trust anchor (genesis or pinned checkpoint)

**URL mutability:** URLs can be shared but not modified. Proof updates require re-generated URLs

**Compression variability:** Actual compression ratios depend on transaction content. High entropy data compresses less

## 11. Conclusion

The rinku network demonstrates that both transactional proofs and account state proofs can be fully self-contained within URLs. By encoding transaction data, ancestry chains, Merkle paths, and checkpoint anchors directly into URLs, we eliminate infrastructure dependencies for verification.

The core innovation is architectural: treating URLs as canonical proof objects rather than references to external state. This enables:

1. **Transaction inclusion proofs** - Prove a payment happened, offline
2. **Account state proofs** - Prove balance/stake at a checkpoint, offline
3. **Verification statelessness** - Verify any proof with only a trust anchor
4. **Indexer elimination** - StatefulReceipts return verified contract state directly to clients, removing the need for indexing infrastructure (The Graph, subgraphs, GraphQL layers) that traditional smart contract platforms depend on

By embracing checkpoint-bounded proofs rather than fighting the impossible battle of proving live state, rinku achieves practical self-provability. The freshness tradeoff (proofs are 15-30 seconds historical) is acceptable for most use cases, while opening entirely new paradigms for trustless verification.

The write-only client model — where clients submit transactions and receive self-proving receipts — means dApps no longer require read infrastructure. The ledger proves facts. Clients hold those proofs. No intermediary is needed.

Ultimately, *the link is the proof* - for both what happened (transactions) and what was true (state).

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

### A.2 Transaction Schema

```typescript
interface Transaction {
  from: string;      // 40-char hex fingerprint
  to: string;        // 40-char hex fingerprint
  amount: string;    // uint64 as decimal string, base units (8 decimals)
  fee: string;       // uint64 as decimal string, base units
  nonce: string;     // uint64 as decimal string, sender sequence number
  tipUrls: string[]; // 0-2 parent references
  ts: string;        // uint64 as decimal string, Unix timestamp (ms)
  sig: string;       // Base64 ECDSA P-256 signature
}
```

**Note:** All numeric values that may exceed 2^53 (`amount`, `fee`, `nonce`, `ts`) are serialized as decimal strings to avoid floating-point precision issues in JavaScript and ensure cross-language compatibility.

### A.3 Proof Bundle Schema

```typescript
interface ProofBundle {
  tx: Transaction;
  fromPubKey: string;       // Base64 ECDSA P-256 public key (SPKI format)
  hash: string;             // SHA-256 hex of canonical tx JSON
  parents: ProofBundle[];   // Recursive ancestry

  // Profile B and C only:
  txInclusion?: {
    merkleIndex: string;    // uint32 as decimal string, leaf position in tx Merkle tree
    merklePath: string[];   // Sibling hashes from leaf to root
  };

  // Profile B and C only:
  checkpoint?: {
    id: string;
    txMerkleRoot: string;   // Root of transaction Merkle tree
    stateRoot: string;      // Root of account state Merkle tree
    height: string;         // uint64 as decimal string, checkpoint height
    blsAggregateSig: string;  // BLS12-381 aggregated signature
    signerBitmap: string;     // Hex bitmap of which validators signed

    // Profile C only:
    validatorProof?: {
      signerLeaves: Array<{
        index: string;      // uint16 as decimal string, position in validator set
        pubKey: string;     // Base64 BLS public key
        weight: string;     // uint64 as decimal string
      }>;
      auxiliaryNodes: Array<{
        level: string;      // uint8 as decimal string, tree level (0 = leaves)
        index: string;      // uint16 as decimal string, position at level
        hash: string;       // SHA-256 hex
        sumWeight: string;  // uint64 as decimal string
      }>;
      rootHash: string;
      totalWeight: string;  // uint64 as decimal string
      threshold: string;    // uint64 as decimal string (>= 2/3 of total)
    };
  };
}
```

### A.4 Account State Proof Schema

```typescript
interface AccountStateProof {
  address: string;           // 40-char hex account address
  checkpoint_height: string; // uint64 as decimal string, checkpoint this proof is anchored to
  checkpoint_hash: string;   // Checkpoint identifier
  state_root: string;        // Merkle root of all account states
  account_state: {
    balance: string;         // uint64 as decimal string, base units
    nonce: string;           // uint64 as decimal string, current transaction nonce
    staked: string;          // uint64 as decimal string, base units
  };
  merkle_proof: {
    leaf_hash: string;       // SHA-256 hex of account state leaf
    path: string[];          // Sibling hashes from leaf to root
    position: string;        // uint32 as decimal string, leaf position in tree
  };
}
```

**Leaf hash computation:**
```
leaf_data = "account:" + address + ":" + normalizeF64(balance) + ":" + nonce + ":" + normalizeF64(staked)
leaf_hash = hex(SHA256(utf8(leaf_data)))
```

Where `normalizeF64(v)` formats the value to exactly 8 decimal places (e.g., `113.06300000`). This string-based format ensures human readability and consistent cross-platform encoding. The leaf data is UTF-8 encoded before hashing.

### A.5 Canonical JSON

Transaction fields are serialized in deterministic order for consistent hashing:
`from, to, amount, fee, nonce, tipUrls, ts, sig`. Canonical JSON = UTF-8, no whitespace, exact numeric encoding rules.

**Signature payload:** The signed payload is the canonical JSON of the transaction **excluding** the `sig` field. The signature is computed over this payload, then appended to the transaction as the `sig` field. This avoids circular dependency where the signature would need to sign itself.

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
    const computedRoot = computeMerkleRoot(
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

// Merkle inclusion proof verification
// Note: leafHash, path[i] are all 64-char hex strings representing 32-byte SHA-256 hashes
function computeMerkleRoot(leafHash, path, index) {
  let current = leafHash;
  let pos = Number(index);  // Parse string to number for index operations
  for (const sibling of path) {
    // Concatenate as raw bytes, not hex strings
    const leftBytes = pos % 2 === 0 ? hexToBytes(current) : hexToBytes(sibling);
    const rightBytes = pos % 2 === 0 ? hexToBytes(sibling) : hexToBytes(current);
    const combined = concatBytes(leftBytes, rightBytes);  // 64 bytes total
    current = sha256Hex(combined);  // Returns 64-char hex string
    pos = Math.floor(pos / 2);
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
async function verifyAccountStateProof(url) {
  // 1. Parse and decode url
  const [, profile, payload] = url.match(/rinku:\/\/(\w+)\/(.+)/);
  if (profile !== 'acct') throw new Error('Not an account proof');
  
  const json = inflate(base64urlDecode(payload));
  const proof = JSON.parse(json);

  // 2. Reconstruct leaf hash from account state (deterministic string encoding)
  // Format: "account:{address}:{balance}:{nonce}:{staked}" with 8 decimal places
  const normalizeF64 = (v) => Number(v).toFixed(8);
  const leafData = `account:${proof.address}:${normalizeF64(proof.account_state.balance)}:${proof.account_state.nonce}:${normalizeF64(proof.account_state.staked)}`;
  const computedLeaf = await sha256Hex(utf8Encode(leafData));
  
  if (computedLeaf !== proof.merkle_proof.leaf_hash) {
    throw new Error('Leaf hash mismatch');
  }

  // 3. Walk Merkle path from leaf to root
  // Note: all hashes are 64-char hex strings representing 32-byte SHA-256 hashes
  let current = computedLeaf;
  let position = Number(proof.merkle_proof.position);  // Parse string to number
  
  for (const sibling of proof.merkle_proof.path) {
    // Concatenate as raw bytes, not hex strings
    const leftBytes = position % 2 === 0 ? hexToBytes(current) : hexToBytes(sibling);
    const rightBytes = position % 2 === 0 ? hexToBytes(sibling) : hexToBytes(current);
    const combined = concatBytes(leftBytes, rightBytes);  // 64 bytes total
    current = await sha256Hex(combined);  // Returns 64-char hex string
    position = Math.floor(position / 2);
  }

  // 4. Verify computed root matches checkpoint state root
  if (current !== proof.state_root) {
    throw new Error('Merkle root mismatch');
  }

  // 5. Return verified state (checkpoint trust assumed or verified separately)
  return {
    valid: true,
    address: proof.address,
    balance: proof.account_state.balance,
    nonce: proof.account_state.nonce,
    staked: proof.account_state.staked,
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

---

