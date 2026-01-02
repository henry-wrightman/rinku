# Rinku: TLDR

> **"rinku: a url-native distributed ledger, where links are the data and the proof."**

---

## What is Rinku?

Rinku is a distributed ledger where **the URL itself is the transaction**. No blockchain nodes needed to verify—just the link.

---

## How URLs Become Transactions

A Rinku transaction URL looks like:
```
https://node.rinku.dev/tx/h/a1b2c3d4...
```

The URL contains everything:
- **Who** sent it (public key)
- **What** they sent (amount, recipient)
- **When** (timestamp, nonce)
- **Proof** it's valid (cryptographic signature)
- **Links** to parent transactions (DAG structure)

The entire transaction is compressed, encoded, and embedded in the URL itself.

---

## How Are Links Secured?

### 1. Ed25519 Signatures
Every transaction is signed with the sender's private key. Anyone can verify the signature using the public key embedded in the URL—no trusted third party needed.

### 2. Hash Chains
Each transaction's hash is computed from its contents. Change one bit → completely different hash. This makes tampering mathematically impossible without detection.

### 3. DAG Structure  
Transactions reference their "parent" transactions by hash, forming a Directed Acyclic Graph. This creates an unbreakable chain of custody back to genesis.

```
          [Genesis]
           /     \
      [Tx A]    [Tx B]
         \       /
          [Tx C]  ← references both A and B
```

### 4. Checkpoint Finality
Periodically, validators sign a checkpoint containing a Merkle root of all transactions. Once signed by enough weighted validators, transactions become **final**—irreversible.

### 5. Self-Contained Merkle Proofs
When a transaction references a finalized parent, instead of embedding the full ancestry, it includes a **Merkle proof**. This proof mathematically proves the parent was included in a signed checkpoint—without carrying all the data. URLs stay compact while remaining fully verifiable.

---

## The Trust Model

```
Genesis Block (root of trust)
    ↓
Checkpoints (signed by validators, ~60 second intervals)
    ↓
Transactions (carry proof back to checkpoint)
```

**To verify any transaction:**
1. Decode the URL → get transaction data
2. Recompute the hash → must match
3. Verify the signature → proves sender authorization
4. Follow parent links → back to a finalized checkpoint
5. For finalized ancestors: verify Merkle proof → proves inclusion in checkpoint
6. Verify checkpoint signatures → anchored to genesis

**Result:** Trustless verification from a single URL. No node queries needed.

---

## What Makes It Different?

| Traditional Blockchain | Rinku |
|------------------------|-------|
| Query a node to verify | URL is self-proving |
| Data stored in blocks | Data stored in URLs |
| Need infrastructure | Just need the link |
| Sync entire chain | Follow embedded parents |

---

## Key Properties

- **Self-Crawlable**: Every URL embeds its full ancestry back to the last checkpoint
- **Trustless**: Verify everything cryptographically, no trusted servers
- **Lightweight**: No full chain sync, verify individual transactions
- **Decentralized**: Consensus via weighted DAG, no single coordinator

---

## Sybil Resistance

Weight determines influence. Weight is calculated as:

```
weight = (account_age_days × 0.3) + (balance × 0.7)
```

New accounts with zero balance have zero weight. This prevents spam attacks.

---

## One-Line Summary

**Rinku turns URLs into cryptographically-verifiable transactions that prove their own validity without requiring any external infrastructure.**
