#!/usr/bin/env bash
# Induce partition conflict → heal via merge pipeline → assert balance parity.
# Used by CI (integration.yml) and for local merge correctness checks.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

echo "==> Cascade + orchestrator merge unit tests"
cargo test -p rinku-node --lib merge:: -- --nocapture

echo "==> Partition heal balance parity (merge_e2e)"
cargo test -p rinku-node --test merge_e2e test_partition_heal_balance_parity -- --nocapture

echo "==> Full merge_e2e suite"
cargo test -p rinku-node --test merge_e2e -- --nocapture

echo "Partition heal checks passed."
