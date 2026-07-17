# CI audit allowlist

Hard gates live in `.github/workflows/quality.yml`. Soft `continue-on-error` is not used.

## Rust (`cargo audit`)

Config: [`.cargo/audit.toml`](../../.cargo/audit.toml).

| Advisory | Crate | Why ignored | Exit criteria |
|----------|-------|-------------|---------------|
| RUSTSEC-2026-0119 | hickory-proto 0.24 | Via `libp2p-mdns`; fix needs libp2p bump to hickory ≥0.26 | Upgrade libp2p |
| RUSTSEC-2025-0009 | ring 0.16 | Via `libp2p-tls`/`rcgen` (quic path in lockfile); node uses TCP | Upgrade libp2p |
| RUSTSEC-2026-0104 / 0098 / 0099 | rustls-webpki 0.101 | Same libp2p-tls path | Upgrade libp2p |
| RUSTSEC-2025-0055 | tracing-subscriber 0.2 | Via `ark-relations` (optional `zk` feature) | Upgrade ark-* |

Unmaintained / yanked / unsound **warnings** are reported but do not fail the gate (cargo-audit default). Treat new **vulnerability** findings as blockers: either upgrade or add an ignore **with reason** in both files.

Removed unused `prometheus` 0.13 to clear RUSTSEC-2024-0437 (protobuf 2.x).

## npm (`scripts/check-npm-audit.mjs`)

CI runs `node scripts/check-npm-audit.mjs`, which fails on any **high/critical** finding not listed in [`npm-audit-allowlist.json`](./npm-audit-allowlist.json).

Root `package.json` `overrides` force patched transitive versions where upstream is stuck:

- `underscore` ≥1.13.8 (clears GHSA-qpx9-hpmf-5gmw via jsonpath/bfj/serve)
- `ws` ≥8.21.1 (clears GHSA-58qx-3vcg-4xpx / GHSA-96hv-2xvq-fx4p via circomlibjs)

Explorer `vite` is bumped to `^6.4.3`.

### Current npm allowlist

| GHSA | Package | Why |
|------|---------|-----|
| GHSA-848j-6mx2-7j84 | elliptic (via circomlibjs → ethers@5) | No patched elliptic; circomlibjs downgrade is breaking. Not on consensus path (`@noble/*` used for protocol crypto). |

If the gate fails on a **new** GHSA, fix or add a dated allowlist entry with reason — do not re-add `continue-on-error`.
