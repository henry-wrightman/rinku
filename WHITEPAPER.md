# Rinku: Self-Provable URLs for Trustless Verification

**Abstract.** We propose a distributed ledger where URLs serve as self-contained cryptographic proofs. This would enable trustless verification without reliance on external infrastructure or services - essentially offline, verifiable transactions. A Rinku URL carries not just transaction data, but its complete verification path; ancestry, signatures, and checkpoint anchors. Ultimately, *the link itself is the proof*. This paper focuses on the URL-native proof system that makes this possible.

## 1. The Problem with Verification Today

Traditional blockchain networks require infrastructure or extraneous software for verification:

1. **Node dependency** - Users must trust a node operator or run their own
2. **State opacity** - Verification requires querying external systems
3. **Proof complexity** - Light clients need specialized tooling and trusted endpoints

Even with light clients, users ultimately must trust *someone else's infrastructure* to provide proofs. The verification is outsourced, not self-contained.

**What if the URL itself could prove its validity?**

## 2. URLs as Proofs

Rinku challenges and inverts the traditional model. Instead of storing data on-chain and fetching proofs from nodes, we encode proofs *directly in URLs*:

e.g
```
rinku://tx/{base64url(deflate(transaction + ancestry + checkpoint))}
```

A Rinku URL contains:
- The transaction data (sender, recipient, amount, signature)
- Ancestry chain back to a finalized checkpoint
- Checkpoint anchor (Merkle root, validator attestations)

**The recipient can verify the entire proof offline.** No RPC calls. No "trusted" infrastructure. Just cryptographic verification of the URL's contents.

## 3. How It Works

### 3.1 Transaction Encoding

Transactions are encoded as compressed JSON directly in the URL path:

```
Transaction → JSON → DEFLATE → Base64url → URL
```

A single transaction URL is roughly 600 characters. With 5 levels of ancestry (proving the transaction chains back to a checkpoint), URLs remain under 1,500 characters which could fit within a QR code.

### 3.2 Proof Structure

A proof bundle contains:

```json
{
  "tx": {
    "from": "a1b2c3...",
    "to": "d4e5f6...",
    "amount": 1000000,
    "nonce": 42,
    "sig": "..."
  },
  "fromPubKey": "BASE64_ECDSA_P256_PUBLIC_KEY",
  "hash": "sha256(tx)",
  "parents": [/* recursive proof bundles */],
  "checkpoint": {
    "id": "cp_789",
    "merkleRoot": "...",
    "height": 1000,
    "blsAggregateSig": "...",
    "signerBitmap": "...",
    "validatorProof": { /* MerkleSumTree multi-proof */ }
  }
}
```

**Key elements:**
- `fromPubKey` - The sender's full ECDSA P-256 public key, enabling signature verification. The `from` field is its fingerprint (SHA-256 truncated to 40 hex chars).
- `checkpoint` - Contains the finality proof: BLS aggregated signature from validators, a bitmap indicating which validators signed, and a Merkle proof for the validator set.

Each parent is itself a proof bundle, creating a recursive structure that traces back to a known checkpoint.

### 3.3 Verification Algorithm

```
function verify(proofUrl):
  bundle = decode(proofUrl)
  
  // 1. Verify public key matches fingerprint
  assert fingerprint(bundle.fromPubKey) == bundle.tx.from
  
  // 2. Verify transaction signature using the public key
  assert ecdsaVerify(bundle.fromPubKey, bundle.tx, bundle.tx.sig)
  
  // 3. Verify hash integrity
  assert sha256(bundle.tx) == bundle.hash
  
  // 4. Recursively verify parents
  for parent in bundle.parents:
    assert verify(parent)
  
  // 5. Verify checkpoint finality
  assert verifyCheckpointSignatures(bundle.checkpoint)
  
  return true
```

The verification is entirely self-contained within the URL.

### 3.4 Data Availability

**Important clarification:** Self-provable URLs guarantee *verification* without infrastructure, not *discovery*. The URL must reach the verifier through some transport:

- Sender shares URL via QR code, message, email etc
- URLs are stored in application databases
- Crawlers can follow parent references to reconstruct comprehensive history

Once the verifier has the URL, no further dependancy on infrastructure is needed. The proof is descrete. This is analogous to a signed document: the signature proves authenticity, but the document must still be delivered.

## 4. Proof Profiles

Different use cases require different security/size tradeoffs:

### Profile A: Receipt (~600-2,300 chars)

**What it proves:** Transaction is valid and chains to a prior checkpoint  
**Trust assumption:** Verifier trusts the checkpoint was correctly signed  
**Use case:** PoS receipts, payment confirmations

```
rinku://tx/{payload}
```

### Profile B: Full Finality (~3,000-10,000 chars)

**What it proves:** Transaction is Merkle-included in a checkpoint signed by ≥67% of validators  
**Trust assumption:** Verifier knows the validator set  
**Use case:** High-value settlements

```
rinku://txp/{payload}
```

### Profile C: Self-Contained (~1,600-2,800 chars)

**What it proves:** Everything in Profile B, plus the validator set commitment itself  
**Trust assumption:** Trust anchor is minimal and can be pinned once (genesis or pinned checkpoint); thereafter proofs are offline-verifiable  
**Use case:** Fully offline verification, air-gapped systems, legal evidence  

```
rinku://sp/{payload}
```

Profile C is the most powerful - it proves finality without any external trust anchors beyond knowing which network you are verifying against.

## 5. Size Analysis

Real-world measurements using high-entropy data (e.g signatures):

| Proof Type | Transactions | URL Length |
|------------|--------------|------------|
| Single tx | 1 | ~600 chars |
| 2-depth ancestry | 3 | ~940 chars |
| 5-depth ancestry | 6 | ~1,400 chars |
| 10-depth ancestry | 11 | ~2,200 chars |

DEFLATE compression achieves 40-55% reduction against the transaction JSON, primarily via repeated field names ("from", "to", "amount") even when values are high-entropy.

### Platform Compatibility

| Platform | Limit | Single tx | 5-depth |
|----------|-------|-----------|---------|
| QR Code (L) | 2,953 bytes | ✓ | ✓ |
| QR Code (H) | 1,273 bytes | ✓ | ✗ |
| Browser URL | 65KB+ | ✓ | ✓ |

Single transactions and short ancestry chains fit in QR codes. Complex proofs use typical URL sharing.

*note* QR capacity depends on QR encoding mode; base64url typically uses byte mode; for maximum density we recommend a QR-optimized encoding (e.g base45) for tx/sp receipts.

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
User → Trust Node → Query Proof → Verify with Node's Help
```

Rinku flow:
```
User → Receive URL → Verify Locally
```

No node operators to trust or rely on. No API endpoints to maintain. No infrastructure that can fail.

### 7.2 Portable Proofs

A Rinku URL can be:
- Printed as a QR code on a receipt
- Sent via SMS, email, or any messaging app
- Embedded in a PDF or document
- Stored offline indefinitely

The proof remains valid as long as the respective cryptography holds.

### 7.3 Offline-First

Verification works completely offline. This enables:
- PoS in areas with poor connectivity
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

## 9. Comparison to Existing Approaches

| Approach | Verification | Trust | Offline |
|----------|--------------|-------|---------|
| Full node | Self-verify | None | ✗ |
| Light client | Query proofs | Node operator | ✗ |
| SPV | Merkle proofs | Header source | ✗ |
| Block explorer | API response | Service provider | ✗ |
| **Rinku URL** | **Self-contained** | **Checkpoint anchor** | **✓** |

Rinku achieves light-client-level verification with full-node-level trust assumptions, all packaged into a portable URL.

## 10. Limitations

**Size constraints:** Complex proofs (30+ transactions) exceed QR capacity and require standard URL sharing.

**Bootstrap requirement:** First-time verifiers need a trust anchor (genesis or pinned checkpoint).

**URL mutability:** URLs can be shared but not modified. Proof updates require re-generated URLs.

**Compression variability:** Actual compression ratios depend on transaction content; high-entropy data compresses less.

## 11. Conclusion

Rinku demonstrates that distributed ledger proofs can be fully self-contained in URLs. By encoding transaction data, ancestry chains, and checkpoint anchors directly in the URL, we eliminate infrastructure dependencies for verification.

The core innovation is architectural: treating URLs as the canonical proof object rather than as references to external state. This enables truly trustless, offline verification - a property no existing blockchain achieves.

*The link is the proof.*

---

## Appendix A: Encoding Specification

### A.1 URL Format

```
rinku://{profile}/{base64url(deflate(json))}
```

Profiles:
- `tx` - Transaction receipt (Profile A)
- `txp` - Transaction with full ancestry (Profile A+)
- `sp` - Self-contained finality proof (Profile C)

### A.2 Transaction Schema

```typescript
interface Transaction {
  from: string;      // 40-char hex fingerprint
  to: string;        // 40-char hex fingerprint
  amount: number;    // Smallest units (8 decimals)
  fee: number;       // Gas fee
  nonce: number;     // Sender sequence number
  tipUrls: string[]; // 0-2 parent references
  ts: number;        // Unix timestamp (ms)
  sig: string;       // ECDSA P-256 signature
}
```

### A.3 Proof Bundle Schema

```typescript
interface ProofBundle {
  tx: Transaction;
  fromPubKey: string;     // Base64 ECDSA P-256 public key (SPKI format)
  hash: string;           // SHA-256 hex of canonical tx JSON
  parents: ProofBundle[]; // Recursive ancestry
  checkpoint: {
    id: string;
    merkleRoot: string;
    height: number;
    blsAggregateSig: string;  // BLS12-381 aggregated signature
    signerBitmap: string;     // Bitmap of which validators signed
    validatorProof: {         // MerkleSumTree multi-proof
      signerLeaves: Array<{ pubKey: string; weight: number }>;
      auxiliaryNodes: string[];
      totalWeight: number;
    };
  };
}
```

### A.4 Canonical JSON

Transaction fields are serialized in deterministic order for consistent hashing:
`from, to, amount, fee, nonce, tipUrls, ts, sig`. 
Canonical JSON = UTF-8, no whitespace, exact numeric encoding rules.

---

## Appendix B: Reference Implementation

A minimal verifier in JavaScript:

```javascript
async function verifyProofUrl(url) {
  // 1. Parse and decode
  const [, profile, payload] = url.match(/rinku:\/\/(\w+)\/(.+)/);
  const json = inflate(base64urlDecode(payload));
  const bundle = JSON.parse(json);
  
  // 2. Verify public key matches fingerprint
  const pubKeyBytes = base64Decode(bundle.fromPubKey);
  const fingerprint = (await sha256(pubKeyBytes)).slice(0, 40);
  if (fingerprint !== bundle.tx.from) {
    throw new Error('Public key does not match sender fingerprint');
  }
  
  // 3. Verify transaction signature using bundled public key
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
  
  // 4. Verify hash integrity
  const hash = await sha256Hex(txBytes);
  if (hash !== bundle.hash) throw new Error('Hash mismatch');
  
  // 5. Verify parents recursively
  for (const parent of bundle.parents) {
    await verifyProofUrl(encodeBundle(parent));
  }
  
  // 6. Verify checkpoint finality (BLS aggregate signature)
  return verifyCheckpointSignatures(bundle.checkpoint);
}
```

---