---
name: consensus-review
description: Review Rinku consensus, merge, slashing, and P2P changes for correctness and security. Use when reviewing PRs touching rinku-node consensus modules, merge logic, validator keys, checkpoint finality, or partition tolerance.
---

# Consensus Review

## Review checklist

Copy and track:

```
- [ ] Balance conservation preserved through merge
- [ ] Quorum threshold unchanged or documented (66.6% default)
- [ ] BLS signature verification on all finality paths
- [ ] No silent fallback on verification failure
- [ ] Deterministic merge ordering (weight tiebreaks tested)
- [ ] Tests added/updated (unit, e2e, or proptest)
- [ ] Dual Rust/TS impl updated if protocol semantics changed
- [ ] No private key material in logs or committed files
```

## High-risk files (read every diff hunk)

- `packages/rinku-node/src/consensus.rs`
- `packages/rinku-node/src/checkpoint.rs`
- `packages/rinku-node/src/merge/`
- `packages/rinku-node/src/slashing.rs`
- `packages/rinku-node/src/sync_verification.rs`
- `packages/rinku-node/src/fast_path.rs`

## Severity labels

- **Critical**: Can cause fork, double-spend, or key leak — block merge
- **High**: Consensus liveness or safety degradation — require test coverage
- **Medium**: Performance or edge-case handling — suggest improvement
- **Low**: Style, logging, docs

## Required test commands after review

```bash
cargo test -p rinku-node --verbose
cargo test -p rinku-node --features zk
bash scripts/test-local-3-nodes.sh --ci
```

## Reference docs

- `docs/architecture/partition-tolerance.md` — merge spec
- `packages/rinku-node/consensus.md` — implementation reference
- `docs/VERSIONING.md` — upgrade lifecycle
