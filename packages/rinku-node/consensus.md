# Rinku Consensus Protocol

## Overview

This document describes Rinku's consensus protocol implementation, designed for mainnet-scale deployment (1,000-10,000+ TPS, billions of wallets).

## Core Components

### 1. Persistent Validator Identity System

**File:** `src/validator_identity.rs`

Validators maintain persistent BLS12-381 keypairs across node restarts:

- **Key Storage:** Validator keys are stored in `validator_keys.json` in the data directory
- **Automatic Generation:** First boot generates and persists new keys
- **Address Derivation:** Validator address derived from BLS public key fingerprint

```rust
pub struct LocalValidatorKeys {
    pub address: String,
    pub bls_private_key: Vec<u8>,
    pub bls_public_key: Vec<u8>,
    pub created_at: u64,
}
```

### 2. Validator Set Management

Validators transition through lifecycle states with epoch-based delays:

| State | Description |
|-------|-------------|
| `PendingActivation` | Registered, waiting for activation delay |
| `Active` | Actively participating in consensus |
| `PendingExit` | Exit initiated, waiting for exit delay |
| `Exited` | No longer participating |
| `Slashed` | Penalized for misbehavior |

**Constants:**
- Minimum stake: 1,000 RKU
- Activation delay: 2 epochs
- Exit delay: 4 epochs
- Epoch length: 60 seconds

### 3. Stake-Weighted Voting

Voting power is proportional to effective stake:

```rust
pub fn voting_power(&self, total_stake: f64) -> f64 {
    if total_stake <= 0.0 || self.effective_stake <= 0.0 {
        return 0.0;
    }
    self.effective_stake / total_stake
}
```

### 4. Quorum Thresholds

**File:** `src/consensus.rs`

Two threshold levels defined:

| Threshold | Value | Purpose |
|-----------|-------|---------|
| `QUORUM_THRESHOLD` | 67% | Minimum for finalization |
| `SUPER_MAJORITY_THRESHOLD` | 75% | Enhanced security threshold |

Checkpoints require 2/3+ stake to finalize.

### 5. Vote Collection & Aggregation

Votes are collected in `VoteAccumulator`:

```rust
pub struct VoteAccumulator {
    pub checkpoint_height: u64,
    pub checkpoint_hash: String,
    pub votes: HashMap<String, Vote>,
    pub total_voting_power: f64,
    pub accumulated_power: f64,
    pub signatures: Vec<Vec<u8>>,
    pub signer_indices: Vec<usize>,
}
```

BLS signatures are aggregated for compact checkpoint proofs.

### 6. Slashing Conditions

**File:** `src/slashing.rs`

Slashable offenses and penalties:

| Offense | Penalty |
|---------|---------|
| Double-signing | 15% of stake |
| Invalid checkpoint | 25% of stake |
| Invalid proof | 20% of stake |
| Invalid witness | 15% of stake |
| Receipt tampering | 25% of stake |
| Liveness failure (3+ misses) | 5% of stake |
| Repeated liveness failure | 10% of stake |

**Double-Sign Evidence:**
```rust
pub struct DoubleSignEvidence {
    pub validator: String,
    pub checkpoint_height: u64,
    pub hash1: String,
    pub hash2: String,
    pub signature1: String,
    pub signature2: Option<String>,
    pub timestamp: u64,
    pub processed: bool,
}
```

Evidence expires after 24 hours if not processed.

### 7. Finality Protocol

**Finality Definition:**
A checkpoint is considered final when:
1. 67%+ of active stake has signed it
2. BLS signatures are verified against validator public keys
3. The checkpoint is added to the finalized checkpoint chain

**Finality Proof:**
```rust
pub struct FinalityProof {
    pub checkpoint_height: u64,
    pub checkpoint_hash: String,
    pub aggregated_signature: Vec<u8>,
    pub signer_bitmap: Vec<u8>,
    pub total_stake_voted: f64,
    pub total_stake: f64,
    pub quorum_threshold: f64,
    pub timestamp: u64,
}
```

### 8. Unbonding Period

Validators exiting stake enter a 14-day unbonding period during which:
- Stake remains slashable for past offenses
- Evidence submitted during unbonding can still slash
- After unbonding, stake is fully released

## Test Coverage

Comprehensive tests for all consensus components:

**Validator Identity Tests (7 tests):**
- Key generation and persistence
- Validator registration with minimum stake
- Activation delay
- Stake addition
- Slashing
- Voting power calculation
- Duplicate registration rejection

**Consensus Tests (8 tests):**
- Vote accumulator quorum
- Duplicate vote rejection
- Super majority threshold
- Finality proof validation
- Vote message determinism
- Vote type differentiation
- Quorum threshold boundary

**Slashing Tests (8 tests):**
- Slash event creation
- Liveness tracking
- All slash reason percentages
- Unbonding queue
- Double-sign evidence submission
- Same hash rejection
- Duplicate evidence rejection
- Evidence processing

## Security Considerations

### Sybil Resistance
Stake-weighted voting prevents Sybil attacks - voting power proportional to economic stake.

### Double-Signing Prevention
Validators signing conflicting checkpoints at the same height are automatically slashed.

### Liveness Enforcement
Missing 3+ consecutive checkpoints triggers slashing, incentivizing uptime.

### Unbonding Security
14-day unbonding allows evidence submission for past misbehavior.

## Recent Improvements (January 2026)

### Epoch Advancement Hook
- ValidatorIdentityService integrated into CheckpointService
- `process_epoch_transition()` called on each checkpoint loop iteration
- Logs epoch transitions with activated/exited validator counts

### Frozen Validator Snapshots
- VoteAccumulator captures validator set at voting round start
- Prevents index drift during voting round
- Deterministic validator ordering (sorted by address)

### Atomic Slashing Integration
- `vote_history` HashMap tracks all votes by (validator, height)
- `slashed_validators` HashSet prevents duplicate slashing
- `reduce_validator_power()` updates both frozen snapshot and accumulated power
- Double-sign detection triggers immediate voting power reduction across all pending rounds

### BLS Verification Enforcement
- Votes with empty BLS public keys are rejected
- All vote signatures verified before acceptance
- 8 new BLS integration tests with real key generation

## Future Improvements

1. **Persistent DAG Storage:** Replace in-memory DAG with persistent storage
2. **State Sharding:** Horizontal scaling for billions of accounts
3. **P2P Protocol:** Replace HTTP gossip with libp2p
4. **Formal Verification:** Mathematical proofs of safety/liveness
5. **Hardware Security:** HSM support for validator keys

## Configuration

Environment variables for consensus tuning:

| Variable | Default | Description |
|----------|---------|-------------|
| `CHECKPOINT_INTERVAL_MS` | 10000 | Time between checkpoints |
| `CHECKPOINT_QUORUM_THRESHOLD` | 0.6666 | Required stake for finality (2/3) |
| `GENESIS_VALIDATORS` | - | Bootstrap validators (addr:pubkey;...) |

## API Endpoints

Consensus-related API endpoints (see `src/api.rs`):

- `GET /api/validators` - List active validators
- `GET /api/checkpoints/:height` - Get checkpoint by height
- `POST /api/gossip` - Submit votes/checkpoints
- `GET /api/finality/metrics` - Finality statistics
