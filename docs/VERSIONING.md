# Rinku Protocol Versioning & Upgrades

This document outlines how protocol upgrades work in Rinku without requiring hard forks.

## Version Format

Rinku uses semantic versioning: `MAJOR.MINOR.PATCH`

| Component | When to increment |
|-----------|-------------------|
| **MAJOR** | Breaking consensus changes |
| **MINOR** | New features, backward compatible |
| **PATCH** | Bug fixes, optimizations |

Current: **Protocol v1.0.0** / Node v0.1.0

## Feature Flags

Features are gated behind activation thresholds. A feature activates when sufficient validator weight signals support.

| Feature | Threshold | Status |
|---------|-----------|--------|
| `bls-aggregation` | 75% | Active |
| `zk-privacy` | 80% | Active |
| `dynamic-gas` | 67% | Active |
| `merkle-sum-tree` | 75% | Active |
| `contract-receipts` | 67% | Active |
| `adaptive-fee-burn` | 75% | Active |

## Upgrade Lifecycle

```
PROPOSED → SIGNALING → LOCKED_IN → ACTIVE
    ↓                      
  REJECTED (if deadline passes without threshold)
```

1. **Proposed**: Upgrade announced with target version and feature set
2. **Signaling**: Validators include version support in checkpoint signatures
3. **Locked-in**: Threshold met, activation scheduled for future height
4. **Active**: Feature enabled network-wide

## API Endpoints

```bash
# Get current version info
GET /api/version

# List all features and their status
GET /api/version/features

# View pending upgrade proposals
GET /api/version/proposals

# Check peer compatibility
GET /api/version/compatibility/:version
```

## For Validators

Validators signal version support automatically via checkpoint signatures:

```typescript
interface ValidatorSignature {
  validator: string;
  signature: string;
  weight: number;
  version?: string;           // Protocol version supported
  supportedFeatures?: string[]; // Feature IDs supported
}
```

## For Node Operators

Check compatibility before upgrading:

```bash
curl http://localhost:3001/api/version/compatibility/1.1.0
```

Response indicates if your node can peer with nodes running the target version.

## Adding New Features

1. Define feature in `packages/core/src/versioning.ts`:

```typescript
export const KNOWN_FEATURES: Record<string, FeatureFlag> = {
  'my-new-feature': {
    id: 'my-new-feature',
    name: 'My New Feature',
    description: 'What it does',
    activationThreshold: 0.75,
  },
  // ...existing features
};
```

2. Guard feature usage in code:

```typescript
if (versionService.isFeatureActive('my-new-feature')) {
  // Use new feature
} else {
  // Fallback behavior
}
```

3. Create upgrade proposal (future checkpoint height):

```typescript
versionService.createProposal({
  targetVersion: '1.1.0',
  features: ['my-new-feature'],
  activationHeight: currentHeight + 1000,
  deadline: Date.now() + 7 * 24 * 60 * 60 * 1000, // 7 days
});
```

## Compatibility Rules

- Nodes with same MAJOR version can peer
- MINOR differences: newer node accepts older transactions
- Minimum compatible version enforced at connection time

## Files

| File | Purpose |
|------|---------|
| `packages/core/src/versioning.ts` | Types, constants, version parsing |
| `packages/node/src/version-service.ts` | Runtime version management |
| `packages/node/src/api.ts` | Version API endpoints |
