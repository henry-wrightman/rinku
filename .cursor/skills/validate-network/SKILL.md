---
name: validate-network
description: Run Rinku testnet validation scripts and interpret results. Use when validating live testnet health, debugging consensus divergence, running validate-testnet or validate-multi-node, or investigating checkpoint/finality issues across nodes.
---

# Validate Network

## Quick start

```bash
# Single-node deep audit (11 sections: DAG, checkpoints, proofs, balances)
npx tsx scripts/validate-testnet.ts https://rinku-genesis.fly.dev

# Cross-node consensus check
npx tsx scripts/validate-multi-node.ts \
  https://rinku-genesis.fly.dev \
  https://rinku-validator-1.fly.dev \
  https://rinku-validator-2.fly.dev

# Local 3-node integration (CI mode)
bash scripts/test-local-3-nodes.sh --ci
```

## Interpreting failures

| Symptom | Likely cause | Check |
|---------|--------------|-------|
| Checkpoint height spread > 2 | Sync lag or leader election stall | `network-health.yml` logs, P2P connectivity |
| Balance mismatch across nodes | Fork or merge bug | `validate-multi-node.ts` account section |
| Merkle root divergence | DAG pruning or tx propagation delay | Compare `dagSize` across nodes |
| Protocol version mismatch | Partial deploy | `fly-deploy.sh status` |
| `CONSENSUS FAILURE` in multi-node report | Critical — do not deploy | `merge/` and `checkpoint.rs` recent changes |

## Live monitoring

```bash
npx tsx scripts/testnet-monitor.ts \
  https://rinku-genesis.fly.dev \
  https://rinku-validator-1.fly.dev \
  https://rinku-validator-2.fly.dev
```

## Activity generation (testnet only — confirm URLs first)

```bash
RINKU_NODE_URLS="https://rinku-genesis.fly.dev,https://rinku-validator-1.fly.dev,https://rinku-validator-2.fly.dev" \
npx tsx scripts/activity-bot-v2.ts --mode=realistic --accounts=10 --duration=300
```

## Report format

When reporting validation results to the user:
1. State which nodes were checked
2. List failed checks with severity (critical vs minor sync lag)
3. Include checkpoint heights and protocol version
4. Recommend next diagnostic step
