# @rinku/zk - Zero-Knowledge Privacy Layer

Privacy-preserving transaction proofs for Rinku using Groth16 ZK-SNARKs.

## Overview

This package enables `rinku://zk/{payload}` URLs that prove transaction validity without revealing:
- Sender address
- Recipient address  
- Transaction amount
- Which specific transaction in the checkpoint

## Installation

```bash
npm install @rinku/zk
```

### Circuit Compilation (Development Only)

To compile the circuits, you need the Circom compiler installed:

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 https://sh.rustup.rs -sSf | sh

# Clone and build circom
git clone https://github.com/iden3/circom.git
cd circom
cargo build --release
cargo install --path circom

# Verify installation
circom --version
```

Then compile the circuits:

```bash
npm run compile:circuits
```

## Usage

### Generating a ZK Proof

```typescript
import { initProver, generateZkUrl } from '@rinku/zk';

// Initialize prover with compiled circuit artifacts
await initProver('./build/circuit.wasm', './build/circuit.zkey');

// Generate proof
const zkUrl = await generateZkUrl({
  txHash: BigInt('0x...'),
  senderPrivKey: BigInt('0x...'),
  // ... other inputs
}, validatorRoot, totalWeight);

// Share zkUrl - it's a self-contained proof
console.log(zkUrl); // rinku://zk/...
```

### Verifying a ZK Proof

```typescript
import { initVerifier, verifyZkUrl } from '@rinku/zk';

// Initialize verifier with verification key
await initVerifier('./build/verification_key.json');

// Verify proof (works offline!)
const result = await verifyZkUrl('rinku://zk/...');
if (result.valid) {
  console.log('Proof is valid!');
  console.log('Checkpoint height:', result.cpHeight);
}
```

## Architecture

### Cryptographic Primitives

| Component | Implementation |
|-----------|---------------|
| ZK System | Groth16 (snarkjs) |
| Hash | Poseidon (circomlibjs) |
| Signatures | EdDSA on BabyJubJub |
| Commitments | Poseidon-based |

### Circuit Constraints

1. **Merkle Inclusion**: Proves tx exists in checkpoint without revealing which one
2. **Signature Verification**: Proves transaction was signed by valid key
3. **Nullifier Derivation**: Prevents double-claiming same proof
4. **Amount Commitment**: Hides transaction amount
5. **Chain ID Binding**: Prevents cross-chain replay

## Proof Size

| Component | Size |
|-----------|------|
| Groth16 proof | 192 bytes |
| Public inputs | ~256 bytes |
| Total (compressed) | ~500 chars |

Fits in QR Code Version 15 (1,250 chars).

## Security

- **Nullifier Registry**: Prevents double-claims
- **Chain ID Binding**: Prevents replay across networks
- **Trusted Setup**: Requires MPC ceremony for production
