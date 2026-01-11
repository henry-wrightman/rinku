# Rinku: Self-Provable URLs for Trustless Verification

**Abstract.** We propose a distributed ledger in which URLs serve as self-contained cryptographic proofs. This would enable trustless verification without reliance on external infrastructure or services. A Rinku URL carries not just transaction data, but its complete verification path - ancestry, signatures, and checkpoint anchors. Ultimately, *the link itself is the proof*. This paper focuses on the URL-native proof system that a distributed network could incorporate.

## 1. The Problem with Verification Today

Traditional blockchain networks require infrastructure or extraneous software for verification:

1. **Node dependency** - Users must trust a node operator or run their own
2. **State opacity** - Verification requires querying external systems
3. **Proof complexity** - Light clients need specific tooling and trusted endpoints

Even with light clients, users ultimately must trust *someone else's infrastructure* to provide proofs. The verification is outsourced, not self-contained.

**What if the URL itself could prove its validity?**

## 2. URLs as Proofs

Instead of storing data on-chain and fetching proofs from nodes, we encode proofs directly into URLs. The profile determines what data is included:

```
rinku://tx/{payload}   # Profile A: transaction only
rinku://txp/{payload}  # Profile B: transaction + ancestry
rinku://sp/{payload}   # Profile C: self-contained finality
```

A full proof URL (Profile B/C) contains:

- The transaction data (sender, recipient, amount, signature)
- Ancestry chain back to a finalized checkpoint
- Checkpoint anchor (Merkle root, validator attestations)

## 3. How It Works

### 3.1 Transaction Encoding

Transactions are encoded as compressed JSON directly in the URL path:

`Transaction -> JSON -> DEFLATE -> Base64url -> URL`

A single transaction URL (Profile A) is roughly 600 characters. Profile B proofs with shallow ancestry (5 levels) remain under 1,500 characters, fitting within a QR code. Full Profile C proofs with finality certificates are larger (see §5 Size Analysis).

### 3.2 Proof Structure

A proof bundle's structure varies by profile. The base structure (all profiles):

```json
{
  "tx": {
    "from": "a1b2c3...",
    "to": "d4e5f6...",
    "amount": "1000000",
    "nonce": "42",
    "sig": "..."
  },
  "fromPubKey": "BASE64_ECDSA_P256_PUBLIC_KEY",
  "hash": "sha256(tx)"
}
```

**Profile B adds** ancestry, checkpoint anchor, and Merkle inclusion path:

```json
{
  "parents": [/* recursive proof bundles */],
  "checkpoint": {
    "id": "cp_789",
    "txMerkleRoot": "...",
    "height": "1000",
    "txMerklePath": ["sibling_hash_1", "sibling_hash_2", ...],
    "txMerkleIndex": 42
  }
}
```

**Profile C adds** full finality certificate:

```json
{
  "checkpoint": {
    "id": "cp_789",
    "txMerkleRoot": "...",
    "height": "1000",
    "blsAggregateSig": "...",
    "signerBitmap": "...",
    "validatorProof": { /* MerkleSumTree multi-proof */ }
  }
}
```

**Key elements:**

- `fromPubKey` - The sender's full ECDSA P-256 public key, enabling signature verification. The `from` field is its fingerprint (SHA-256 truncated to 40 hex chars).
- `checkpoint` - Profile B includes the checkpoint anchor; Profile C adds the BLS aggregated signature, signer bitmap, and validator set proof.

Each parent is itself a proof bundle, creating a recursive structure that traces back to a known checkpoint.

### 3.3 Verification Algorithm

```
function verify(proofUrl):
  bundle, profile = decode(proofUrl)
  
  // 1. Verify public key matches fingerprint
  assert fingerprint(bundle.fromPubKey) == bundle.tx.from
  
  // 2. Verify transaction signature via the public key
  assert ecdsaVerify(bundle.fromPubKey, bundle.tx, bundle.tx.sig)
  
  // 3. Verify hash integrity
  assert sha256(bundle.tx) == bundle.hash
  
  // 4. Profile-specific verification
  if profile == "tx":
    return true  // Profile A: authorization only
  
  // 5. Verify ancestry (Profile B, C)
  for parent in bundle.parents:
    assert verify(parent)
  
  if profile == "txp":
    // Profile B: verify Merkle inclusion, then checkpoint trust
    computedRoot = merkleRoot(bundle.hash, bundle.checkpoint.txMerklePath, bundle.checkpoint.txMerkleIndex)
    assert computedRoot == bundle.checkpoint.txMerkleRoot
    assert checkpointIsTrusted(bundle.checkpoint.id)
    return true
  
  // 6. Profile C: verify Merkle inclusion first
  computedRoot = merkleRoot(bundle.hash, bundle.checkpoint.txMerklePath, bundle.checkpoint.txMerkleIndex)
  assert computedRoot == bundle.checkpoint.txMerkleRoot
  
  // 7. Verify BLS aggregate signature
  assert verifyBLSAggregate(bundle.checkpoint)
  
  // 8. Verify validator proof and derive quorum from totalWeight (not from prover)
  assert verifyValidatorProof(bundle.checkpoint.validatorProof)
  signerWeight = sum(signerLeaves.map(l => l.weight))
  totalWeight = bundle.checkpoint.validatorProof.totalWeight
  requiredWeight = floor(totalWeight * 2 / 3)  // Derived, not trusted from prover
  assert signerWeight >= requiredWeight
  return true
```

### 3.4 Data Availability

**Important clarification:** Self-provable URLs guarantee *verification* without infrastructure, not *discovery*. The URL must reach the verifier through some transport:

- Sender shares URL via QR code, message, email etc
- URLs are stored in application databases
- Crawlers can follow parent references to reconstruct comprehensive history

Once the verifier has the URL, no further dependency on infrastructure is needed. The proof is discrete. This is analogous to a signed document: the signature proves authenticity, but the document must still be delivered.

## 4. Proof Profiles

Different use cases require different security/size tradeoffs. Profiles are defined by what data is included:

### Profile A: Authorization (`tx`) - ~600 characters

**Contains:** Transaction + sender public key + signature + hash (no ancestry, no checkpoint)
**What it proves:** Transaction is validly signed by the sender
**Trust assumption:** None beyond trusting the sender's key binding (fingerprint → pubkey)
**Use case:** Payment authorizations, intent receipts (note: does not prove finalization)

```
rinku://tx/{payload}
```

### Profile B: Full Ancestry (`txp`) - ~800 - 2,300 characters

**Contains:** Transaction + recursive parent proofs + checkpoint anchor (id, txMerkleRoot, height) + Merkle inclusion path
**What it proves:** Transaction is Merkle-included in a checkpoint; finality depends on trusted checkpoint chain
**Trust assumption:** Verifier trusts the checkpoint anchor (via bootstrapped trust or known validator set)
**Use case:** High-value settlements, audit trails

```
rinku://txp/{payload}
```

### Profile C: Self-Contained (`sp`) - ~2,500 - 5,000+ characters

**Contains:** Everything in Profile B + full finality certificate (BLS aggregate sig + signer bitmap + validator multi-proof)
**What it proves:** Complete finality with validator set commitment
**Trust assumption:** Requires pinned chain identity + initial trust anchor (genesis or pinned checkpoint); after bootstrapping, proofs verify offline
**Use case:** Fully offline verification, air-gapped systems, legal evidence

```
rinku://sp/{payload}
```

Profile C size scales with validator committee size. With packed binary encoding + DEFLATE:
- N=16 validators, 11 signers: ~2,500 bytes (fits QR-L)
- N=21 validators, 14 signers: ~3,400 bytes (URL sharing)
- N=32 validators, 22 signers: ~5,100 bytes (URL sharing)

Profile C is the most powerful - once bootstrapped with a trust anchor, all subsequent proofs verify completely offline.

## 5. Size Analysis

Real-world measurements using high-entropy data (random hex hashes, ECDSA signatures):

### Profile A/B (without finality certificate)

| Proof Type | Transactions | URL Length |
|------------|--------------|------------|
| Single tx (Profile A) | 1 | ~600 chars |
| 2-depth ancestry | 3 | ~940 chars |
| 5-depth ancestry | 6 | ~1,420 chars |
| 10-depth ancestry | 11 | ~2,230 chars |

### Profile C (with finality certificate, packed binary + DEFLATE)

| Committee | Signers | Compressed Size | QR-L Compatible |
|-----------|---------|-----------------|-----------------|
| N=16 | k=11 | ~2,500 bytes | ✓ |
| N=21 | k=14 | ~3,400 bytes | ✗ (URL sharing) |
| N=32 | k=22 | ~5,100 bytes | ✗ (URL sharing) |
| N=64 | k=43 | ~10,700 bytes | ✗ (URL sharing) |

### Platform Compatibility

| Platform | Limit | Profile A | Profile B (5-depth) | Profile C (N=16) |
|----------|-------|-----------|---------------------|------------------|
| QR Code (L) | 2,953 bytes | ✓ | ✓ | ✓ |
| QR Code (H) | 1,273 bytes | ✓ | ✗ | ✗ |
| Browser URL | 65KB+ | ✓ | ✓ | ✓ |

Profile A/B receipts with shallow ancestry fit in QR codes. Profile C with small committees (≤16 validators) fits QR-L; larger committees require URL sharing.

*Note: QR capacity depends on encoding mode; base64url requires byte mode. For maximum density, consider QR-optimized encoding (e.g., base45) for compact receipts.*

## 6. Trust Bootstrapping

A fresh verifier needs a trust anchor:

1. **Genesis trust** - Know the genesis validator set
2. **Checkpoint chain** - Each checkpoint commits to the next validator set
3. **Pinned checkpoint** - Trust a recent checkpoint from an out-of-band source

This mirrors TLS certificate chains: the proof is self-contained, but root trust must be established externally. Once bootstrapped, all subsequent proofs verify offline.

## 7. Why This Matters

### 7.1 Infrastructure Independence

Traditional flow:

```
User -> Trust Node -> Query Proof -> Verify with Node's Help
```

Rinku flow:

```
User -> Receive URL -> Verify Locally
```

### 7.2 Portable Proofs

A Rinku URL can be:

- Printed as a QR code on a receipt
- Sent via SMS, email, or any messaging app
- Embedded in a PDF or document
- Stored offline indefinitely

The proof remains valid as long as the respective cryptography holds.

### 7.3 Offline-First

Verification works completely offline. This enables:

- POS terminals in areas with poor connectivity
- Air-gapped security systems
- Archival verification that withstands over large periods of time
- Cross-border payments without infrastructure

## 8. Cryptographic Primitives

The proof system uses standard, well-audited cryptography:

| Component | Algorithm | Purpose |
|-----------|-----------|---------|
| Transaction signatures | ECDSA P-256 | Sender authorization |
| Hash function | SHA-256 | Transaction identity, Merkle trees |
| Validator signatures | BLS12-381 | Aggregated checkpoint attestation |
| Compression | DEFLATE | URL size reduction |
| Encoding | Base64url | URL-safe representation |

ECDSA verification can use the Web Crypto API; BLS verification can use a WASM library (e.g blst)

## 9. Limitations

**Size constraints:** Complex proofs (30+ transactions) exceed QR capacity and require standard URL sharing.

**Bootstrap requirement:** First-time verifiers need a trust anchor (genesis or pinned checkpoint).

**URL mutability:** URLs can be shared but not modified. Proof updates require re-generated URLs.

**Compression variability:** Actual compression ratios depend on transaction content; high-entropy data compresses less.

**Verifier safety limits:** Implementations should enforce DoS protection:
- Maximum recursion depth: 32 levels
- Maximum decoded payload: 1MB
- Maximum parents per transaction: 2
- Maximum total proof bundle size: 64KB

## 10. Conclusion

Rinku demonstrates that distributed ledger proofs can be fully self-contained within URLs. By encoding transaction data, ancestry chains, and checkpoint anchors directly into the URL, we can eliminate infrastructure dependencies for verification.

The core innovation is architectural: treating URLs as the canonical proof object rather than as references to external state. This enables truly trustless, offline verification - a property no existing blockchain currently supports.

*The link is the proof.*

---

## Appendix A: Encoding Specification

### A.1 URL Format

```
rinku://{profile}/{base64url(deflate(json))}
```

Profiles:

- `tx` - Transaction receipt (Profile A)
- `txp` - Transaction with full ancestry (Profile B)
- `sp` - Self-contained finality proof (Profile C)

### A.2 Transaction Schema

```typescript
interface Transaction {
  from: string;      // 40-char hex fingerprint
  to: string;        // 40-char hex fingerprint
  amount: uint64;    // Smallest units (8 decimals)
  fee: uint64;       // Gas fee
  nonce: uint64;     // Sender sequence number
  tipUrls: string[]; // 0-2 parent references
  ts: uint64;        // Unix timestamp (ms)
  sig: string;       // ECDSA P-256 signature
}
```

*Note: Integer fields are encoded as decimal strings in canonical JSON to ensure cross-language consistency.*

### A.3 Proof Bundle Schema

*Note: All integer fields are encoded as decimal strings in canonical JSON.*

```typescript
// Base bundle (Profile A)
interface ProofBundleA {
  tx: Transaction;
  fromPubKey: string;     // Base64 ECDSA P-256 public key (SPKI format)
  hash: string;           // SHA-256 hex of canonical tx JSON
}

// Profile B adds ancestry + checkpoint anchor + inclusion proof
interface ProofBundleB extends ProofBundleA {
  parents: ProofBundleB[];  // Recursive ancestry
  checkpoint: {
    id: string;
    txMerkleRoot: string;   // Merkle root of finalized transactions
    height: uint64;
    txMerklePath: string[]; // Sibling hashes for inclusion proof
    txMerkleIndex: uint16;  // Leaf position in tree
  };
}

// Profile C adds full finality certificate
interface ProofBundleC extends ProofBundleA {
  parents: ProofBundleC[];
  checkpoint: {
    id: string;
    txMerkleRoot: string;
    height: uint64;
    txMerklePath: string[];
    txMerkleIndex: uint16;
    blsAggregateSig: string;    // BLS12-381 aggregated signature
    signerBitmap: string;       // Bitmap of which validators signed
    validatorProof: {           // MerkleSumTree multi-proof
      // signerLeaves ordered by increasing index from signerBitmap popcount walk
      signerLeaves: Array<{
        index: uint16;        // Position in validator set (0-based)
        address: string;      // Validator address (40-char hex)
        blsPublicKey: string; // Base64url-encoded BLS12-381 public key
        weight: uint64;       // Stake weight
      }>;
      // auxiliaryNodes with explicit (level, index) for deterministic reconstruction
      auxiliaryNodes: Array<{
        level: uint8;         // Tree level (0 = leaf layer)
        index: uint16;        // Position within level
        hash: string;         // Node hash (64-char hex)
        sumWeight: uint64;    // Accumulated weight in subtree
      }>;
      totalWeight: uint64;    // Derived from sum tree root (verifier computes quorum)
      rootHash: string;       // Validator sum tree root hash for verification
    };
  };
}
```

### A.4 Canonical JSON

Transaction fields are serialized in deterministic order for consistent hashing:
`from, to, amount, fee, nonce, tipUrls, ts, sig`. Canonical JSON = UTF-8, no whitespace, exact numeric encoding rules.

*Note: `hash` is computed over the `tx` object only (the Transaction fields), not including bundle-level fields like `fromPubKey` or `parents`.*

---

## Appendix B: Reference Implementation

A minimal verifier in JavaScript:

```javascript
// Entry point: verify a proof URL
async function verifyProofUrl(url) {
  const [, profile, payload] = url.match(/rinku:\/\/(\w+)\/(.+)/);
  const json = inflate(base64urlDecode(payload));
  const bundle = JSON.parse(json);
  return verifyBundle(bundle, profile);
}

// Core verification logic (works on decoded bundles)
async function verifyBundle(bundle, profile) {
  // 1. Verify public key matches fingerprint
  const pubKeyBytes = base64Decode(bundle.fromPubKey);
  const hashBytes = await sha256(pubKeyBytes);  // Returns Uint8Array
  const hashHex = bytesToHex(hashBytes);        // Convert to 64-char hex string
  const fingerprint = hashHex.slice(0, 40);     // First 20 bytes = 40 hex chars
  if (fingerprint !== bundle.tx.from) {
    throw new Error('Public key does not match sender fingerprint');
  }
  
  // 2. Verify transaction signature using bundled public key
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
  
  // 4. Profile A: authorization only (no ancestry/checkpoint)
  if (profile === 'tx') return true;
  
  // 5. Verify ancestry (Profile B, C)
  if (bundle.parents) {
    for (const parent of bundle.parents) {
      await verifyBundle(parent, profile);
    }
  }
  
  // 6. Profile B: verify Merkle inclusion + checkpoint trust
  if (profile === 'txp') {
    const computedRoot = merkleRoot(bundle.hash, bundle.checkpoint.txMerklePath, bundle.checkpoint.txMerkleIndex);
    if (computedRoot !== bundle.checkpoint.txMerkleRoot) {
      throw new Error('Merkle inclusion verification failed');
    }
    return checkpointIsTrusted(bundle.checkpoint.id);
  }
  
  // 7. Profile C: verify Merkle inclusion + full finality certificate
  const computedRoot = merkleRoot(bundle.hash, bundle.checkpoint.txMerklePath, bundle.checkpoint.txMerkleIndex);
  if (computedRoot !== bundle.checkpoint.txMerkleRoot) {
    throw new Error('Merkle inclusion verification failed');
  }
  
  // 8. Verify BLS signature + validator proof + quorum
  const blsValid = await verifyBLSAggregate(bundle.checkpoint);
  const validatorResult = verifyValidatorProof(bundle.checkpoint.validatorProof);
  const signerWeight = validatorResult.signerWeight;
  const totalWeight = validatorResult.totalWeight;
  const requiredWeight = Math.floor(totalWeight * 2 / 3);  // Derived from sum tree
  
  return blsValid && validatorResult.valid && signerWeight >= requiredWeight;
}
```

---

## References

1. D. Boneh, M. Drijvers, and G. Neven. "Compact Multi-Signatures for Smaller Blockchains." ASIACRYPT 2018. https://crypto.stanford.edu/~dabo/pubs/papers/BLSmultisig.html
2. D. Boneh, B. Lynn, and H. Shacham. "Short Signatures from the Weil Pairing." Journal of Cryptology, 2004.
3. R. Merkle. "A Digital Signature Based on a Conventional Encryption Function." CRYPTO 1987.

---

