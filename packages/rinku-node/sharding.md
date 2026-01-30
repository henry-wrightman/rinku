# Rinku Sharding Strategy for Billions of Accounts

## Overview

This document outlines the sharding architecture for scaling Rinku to billions of accounts while maintaining the core innovations of URL-native proofs and DAG-based consensus.

## Design Principles

1. **Account-based sharding** - Accounts are partitioned by address prefix
2. **Cross-shard atomicity** - Multi-shard transactions use 2-phase commit
3. **Shard-local proofs** - Self-provable URLs work within shards
4. **Dynamic rebalancing** - Shards can split/merge based on load

## Shard Architecture

### Shard Identification

```
Shard ID = sha256(account_address)[0:2]  // 65536 possible shards
```

The first 2 bytes of the address hash determine shard assignment. This provides:
- Uniform distribution across shards
- Deterministic routing (no lookup required)
- 65,536 maximum shards (billions of accounts / 65K = ~15K accounts/shard at 1B scale)

### Shard Structure

Each shard maintains:
- Local account state (balances, nonces, stakes)
- Local DAG (transactions within shard)
- Local checkpoints (finalized shard state)
- Sparse Merkle trie (shard state root)
- Cross-shard receipt queue

### Shard Groups

Shards are organized into groups for efficient consensus:

```
Shard Group 0: Shards 0x0000 - 0x00FF (256 shards)
Shard Group 1: Shards 0x0100 - 0x01FF (256 shards)
...
Shard Group 255: Shards 0xFF00 - 0xFFFF (256 shards)
```

Each shard group shares a validator set for checkpoint signing.

## Cross-Shard Transactions

### Single-Shard Transactions (Fast Path)

Transactions where sender and receiver are in the same shard:
1. Validate locally
2. Add to local DAG
3. Finalize in shard checkpoint
4. Generate shard-local proof

### Cross-Shard Transactions (2-Phase Commit)

Transactions spanning multiple shards use receipts:

**Phase 1: Lock**
```
Source Shard:
1. Validate sender balance/nonce
2. Lock sender funds
3. Create CrossShardReceipt with proof
4. Emit receipt to destination shard
```

**Phase 2: Apply**
```
Destination Shard:
1. Verify CrossShardReceipt proof
2. Credit receiver
3. Create completion receipt
4. Emit back to source shard

Source Shard:
1. Verify completion receipt
2. Finalize sender deduction
3. Release lock
```

### Receipt Format

```rust
struct CrossShardReceipt {
    source_shard: u16,
    dest_shard: u16,
    tx_hash: [u8; 32],
    sender: String,
    receiver: String,
    amount: u64,
    nonce: u64,
    source_state_root: [u8; 32],
    merkle_proof: Vec<[u8; 32]>,
    signature: Vec<u8>,
}
```

## Global State Root

### Shard State Tree

All shard roots are aggregated into a global state tree:

```
Global Root
├── Shard 0x0000 Root
├── Shard 0x0001 Root
├── ...
└── Shard 0xFFFF Root
```

The global root is computed every N checkpoints (global epoch).

### Beacon Chain

A lightweight beacon chain coordinates:
- Global epoch progression
- Shard committee assignments
- Cross-shard receipt routing
- Global state root aggregation

## Proof Architecture

### Intra-Shard Proofs

Standard self-provable URLs work within shards:
```
rinku://sp/v1/<shard_id>/<checkpoint>/<proof>
```

### Cross-Shard Proofs

Cross-shard proofs include:
1. Source shard proof (sender deduction)
2. Cross-shard receipt
3. Destination shard proof (receiver credit)
4. Global state root (optional, for full verification)

```
rinku://xsp/v1/<source_shard>/<dest_shard>/<proof>
```

## Validator Assignment

### Shard Committees

Each shard group has a rotating validator committee:
- Committee size: 64-128 validators per shard group
- Rotation period: 1 epoch (6 hours)
- Selection: Stake-weighted random using beacon chain randomness

### Stake Requirements

```
Minimum stake per shard: 1000 RKU
Maximum shards per validator: 8 (to prevent centralization)
Total network validators target: 10,000+
```

## Data Availability

### Shard Data

Each shard node stores:
- Full shard state
- 100 checkpoints of shard history
- Cross-shard receipt inbox/outbox

### Light Clients

Light clients can verify:
- Account balance proofs (shard Merkle proof + global root)
- Transaction inclusion (shard DAG proof)
- Cross-shard receipts (source + dest proofs)

## Migration Path

### Phase 1: Single Shard (Current)
- All accounts in shard 0
- Full node stores everything
- Standard self-provable URLs

### Phase 2: Shard Groups (4-16 shards)
- Split by address prefix
- Validator committees per group
- Cross-shard receipts enabled

### Phase 3: Full Sharding (256+ shards)
- Dynamic shard count based on load
- Beacon chain coordination
- Global state root aggregation

### Phase 4: Unlimited Scaling (1000+ shards)
- Auto-splitting hot shards
- Shard merging for cold shards
- Cross-shard proof optimization

## Performance Targets

| Metric | Single Shard | 16 Shards | 256 Shards | 4096 Shards |
|--------|--------------|-----------|------------|-------------|
| TPS | 1,000 | 10,000 | 100,000 | 1,000,000 |
| Accounts | 10M | 100M | 1B | 10B+ |
| State Size | 100GB | 10GB/shard | 5GB/shard | 3GB/shard |
| Finality | 15s | 15s | 30s (cross) | 45s (cross) |

## Implementation Priority

1. **Shard-aware storage** (redb column families)
2. **Address-based routing**
3. **Cross-shard receipt queue**
4. **Shard committee selection**
5. **Beacon chain prototype**
6. **Global state aggregation**

## Open Questions

1. How to handle smart contracts that span multiple shards?
2. Optimal shard split/merge thresholds?
3. Cross-shard transaction fee distribution?
4. Shard state proof compression for mobile clients?

## References

- Ethereum 2.0 Sharding Specification
- Zilliqa Sharding Architecture
- Near Protocol Nightshade Sharding
- Polkadot Parachains Model
