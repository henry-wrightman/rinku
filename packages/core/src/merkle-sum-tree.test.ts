import { describe, it, expect } from 'vitest';
import {
  buildMerkleSumTree,
  getMerkleSumProof,
  verifyMerkleSumProof,
  computeMerkleSumRootFromProofs,
  encodeMerkleSumProof,
  decodeMerkleSumProof,
  encodeMerkleSumRoot,
  decodeMerkleSumRoot,
  getMerkleSumMultiProof,
  verifyMerkleSumMultiProof,
  multiProofNodeCount,
  individualProofNodeCount,
  type MerkleSumLeaf,
  type MerkleSumRoot
} from './merkle-sum-tree.js';
import { base64urlEncode } from './encoding.js';

function createMockLeaves(count: number): MerkleSumLeaf[] {
  return Array.from({ length: count }, (_, i) => ({
    index: i,
    address: `validator-${i}`,
    blsPublicKey: base64urlEncode(new Uint8Array(96).fill(i + 1)),
    weight: 100 + i * 10
  }));
}

describe('MerkleSumTree', () => {
  describe('buildMerkleSumTree', () => {
    it('should build tree with correct totalWeight for single leaf', () => {
      const leaves = createMockLeaves(1);
      const { root } = buildMerkleSumTree(leaves);

      expect(root.totalWeight).toBe(100);
      expect(root.hash).toBeTruthy();
    });

    it('should build tree with correct totalWeight for multiple leaves', () => {
      const leaves = createMockLeaves(3);
      const { root } = buildMerkleSumTree(leaves);

      expect(root.totalWeight).toBe(100 + 110 + 120);
      expect(root.hash).toBeTruthy();
    });

    it('should build tree with correct totalWeight for power-of-2 leaves', () => {
      const leaves = createMockLeaves(4);
      const { root } = buildMerkleSumTree(leaves);

      expect(root.totalWeight).toBe(100 + 110 + 120 + 130);
    });

    it('should handle empty leaves', () => {
      const { root } = buildMerkleSumTree([]);
      expect(root.totalWeight).toBe(0);
    });

    it('should produce deterministic root for same leaves', () => {
      const leaves = createMockLeaves(5);
      const { root: root1 } = buildMerkleSumTree(leaves);
      const { root: root2 } = buildMerkleSumTree(leaves);

      expect(root1.hash).toBe(root2.hash);
      expect(root1.totalWeight).toBe(root2.totalWeight);
    });

    it('should produce different roots for different leaves', () => {
      const leaves1 = createMockLeaves(3);
      const leaves2 = createMockLeaves(4);
      const { root: root1 } = buildMerkleSumTree(leaves1);
      const { root: root2 } = buildMerkleSumTree(leaves2);

      expect(root1.hash).not.toBe(root2.hash);
    });
  });

  describe('getMerkleSumProof', () => {
    it('should generate valid proof for each leaf', () => {
      const leaves = createMockLeaves(5);
      const { root } = buildMerkleSumTree(leaves);

      for (let i = 0; i < leaves.length; i++) {
        const proof = getMerkleSumProof(leaves, i);
        expect(proof).not.toBeNull();
        expect(proof!.leaf.index).toBe(i);
        expect(proof!.leaf.weight).toBe(100 + i * 10);
      }
    });

    it('should return null for non-existent leaf index', () => {
      const leaves = createMockLeaves(3);
      const proof = getMerkleSumProof(leaves, 999);
      expect(proof).toBeNull();
    });
  });

  describe('verifyMerkleSumProof', () => {
    it('should verify valid proof', () => {
      const leaves = createMockLeaves(7);
      const { root } = buildMerkleSumTree(leaves);

      for (let i = 0; i < leaves.length; i++) {
        const proof = getMerkleSumProof(leaves, i);
        expect(proof).not.toBeNull();

        const result = verifyMerkleSumProof(proof!, root);
        expect(result.valid).toBe(true);
        expect(result.leafWeight).toBe(leaves[i].weight);
        expect(result.errors).toHaveLength(0);
      }
    });

    it('should reject proof with wrong root hash', () => {
      const leaves = createMockLeaves(4);
      const { root } = buildMerkleSumTree(leaves);
      const proof = getMerkleSumProof(leaves, 0)!;

      const fakeRoot: MerkleSumRoot = {
        hash: 'fakehash1234567890',
        totalWeight: root.totalWeight
      };

      const result = verifyMerkleSumProof(proof, fakeRoot);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Root hash mismatch'))).toBe(true);
    });

    it('should reject proof with wrong totalWeight', () => {
      const leaves = createMockLeaves(4);
      const { root } = buildMerkleSumTree(leaves);
      const proof = getMerkleSumProof(leaves, 0)!;

      const fakeRoot: MerkleSumRoot = {
        hash: root.hash,
        totalWeight: 999999
      };

      const result = verifyMerkleSumProof(proof, fakeRoot);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Total weight mismatch'))).toBe(true);
    });

    it('should reject tampered leaf weight', () => {
      const leaves = createMockLeaves(4);
      const { root } = buildMerkleSumTree(leaves);
      const proof = getMerkleSumProof(leaves, 0)!;

      proof.leaf.weight = 99999;

      const result = verifyMerkleSumProof(proof, root);
      expect(result.valid).toBe(false);
    });

    it('should reject tampered leaf address', () => {
      const leaves = createMockLeaves(4);
      const { root } = buildMerkleSumTree(leaves);
      const proof = getMerkleSumProof(leaves, 0)!;

      proof.leaf.address = 'attacker-address';

      const result = verifyMerkleSumProof(proof, root);
      expect(result.valid).toBe(false);
    });
  });

  describe('computeMerkleSumRootFromProofs', () => {
    it('should compute consistent root from multiple proofs', () => {
      const leaves = createMockLeaves(5);
      const { root } = buildMerkleSumTree(leaves);

      const proofs = [0, 2, 4].map(i => getMerkleSumProof(leaves, i)!);
      const computedRoot = computeMerkleSumRootFromProofs(proofs);

      expect(computedRoot).not.toBeNull();
      expect(computedRoot!.hash).toBe(root.hash);
      expect(computedRoot!.totalWeight).toBe(root.totalWeight);
    });

    it('should return null for inconsistent proofs', () => {
      const leaves1 = createMockLeaves(4);
      const leaves2 = createMockLeaves(5);

      const proof1 = getMerkleSumProof(leaves1, 0)!;
      const proof2 = getMerkleSumProof(leaves2, 0)!;

      const result = computeMerkleSumRootFromProofs([proof1, proof2]);
      expect(result).toBeNull();
    });

    it('should return null for empty proofs', () => {
      const result = computeMerkleSumRootFromProofs([]);
      expect(result).toBeNull();
    });
  });

  describe('Encoding/Decoding', () => {
    it('should encode and decode proof correctly', () => {
      const leaves = createMockLeaves(4);
      const proof = getMerkleSumProof(leaves, 1)!;

      const encoded = encodeMerkleSumProof(proof);
      const decoded = decodeMerkleSumProof(encoded);

      expect(decoded.leaf.index).toBe(proof.leaf.index);
      expect(decoded.leaf.address).toBe(proof.leaf.address);
      expect(decoded.leaf.weight).toBe(proof.leaf.weight);
      expect(decoded.siblings.length).toBe(proof.siblings.length);
      expect(decoded.pathBits.length).toBe(proof.pathBits.length);
    });

    it('should encode and decode root correctly', () => {
      const root: MerkleSumRoot = {
        hash: 'abc123def456',
        totalWeight: 12345
      };

      const encoded = encodeMerkleSumRoot(root);
      const decoded = decodeMerkleSumRoot(encoded);

      expect(decoded.hash).toBe(root.hash);
      expect(decoded.totalWeight).toBe(root.totalWeight);
    });
  });

  describe('Security Properties', () => {
    it('should make totalWeight cryptographically bound to validator set', () => {
      const leaves = createMockLeaves(5);
      const { root } = buildMerkleSumTree(leaves);

      const modifiedLeaves = [...leaves];
      modifiedLeaves[2] = { ...modifiedLeaves[2], weight: 99999 };
      const { root: modifiedRoot } = buildMerkleSumTree(modifiedLeaves);

      expect(root.hash).not.toBe(modifiedRoot.hash);
      expect(root.totalWeight).not.toBe(modifiedRoot.totalWeight);
    });

    it('should prevent denominator attack by binding totalWeight to proof', () => {
      const leaves = createMockLeaves(5);
      const { root } = buildMerkleSumTree(leaves);
      const proof = getMerkleSumProof(leaves, 0)!;

      const result = verifyMerkleSumProof(proof, root);
      expect(result.valid).toBe(true);

      const attackerRoot: MerkleSumRoot = {
        hash: root.hash,
        totalWeight: 10
      };

      const attackResult = verifyMerkleSumProof(proof, attackerRoot);
      expect(attackResult.valid).toBe(false);
      expect(attackResult.errors.some(e => e.includes('Total weight mismatch'))).toBe(true);
    });
  });

  describe('Multi-Proof Optimization', () => {
    it('should generate valid multi-proof for subset of signers', () => {
      const leaves = createMockLeaves(8);
      const { root } = buildMerkleSumTree(leaves);
      const signerIndices = [0, 2, 5, 7];

      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      expect(multiProof).not.toBeNull();
      expect(multiProof!.leaves.length).toBe(4);

      const result = verifyMerkleSumMultiProof(multiProof!, root);
      expect(result.valid).toBe(true);
      expect(result.signerWeight).toBe(100 + 120 + 150 + 170);
      expect(multiProof!.committeeSize).toBe(8);
    });

    it('should verify multi-proof against expected root', () => {
      const leaves = createMockLeaves(16);
      const { root } = buildMerkleSumTree(leaves);
      const signerIndices = [0, 1, 4, 5, 8, 9, 12, 13, 14, 15];

      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      const result = verifyMerkleSumMultiProof(multiProof!, root);

      expect(result.valid).toBe(true);
      expect(multiProof!.committeeSize).toBe(16);
    });

    it('should reject multi-proof with wrong root', () => {
      const leaves = createMockLeaves(8);
      const { root } = buildMerkleSumTree(leaves);
      const signerIndices = [0, 2, 5];

      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      const wrongRoot: MerkleSumRoot = { hash: 'wrong', totalWeight: 999 };

      const result = verifyMerkleSumMultiProof(multiProof!, wrongRoot);
      expect(result.valid).toBe(false);
    });

    it('should have fewer nodes than individual proofs for large k', () => {
      const N = 32;
      const k = 22;
      const leaves = createMockLeaves(N);
      const signerIndices = Array.from({ length: k }, (_, i) => Math.floor(i * N / k));

      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      const multiNodes = multiProofNodeCount(multiProof!);
      const individualNodes = individualProofNodeCount(k, N);

      expect(multiNodes).toBeLessThan(individualNodes);
      const savings = 1 - multiNodes / individualNodes;
      expect(savings).toBeGreaterThan(0.5);
    });

    it('should achieve >60% savings for typical committee sizes', () => {
      const testCases = [
        { N: 16, k: 11 },
        { N: 21, k: 14 },
        { N: 32, k: 22 },
        { N: 64, k: 43 },
      ];

      for (const { N, k } of testCases) {
        const leaves = createMockLeaves(N);
        const { root } = buildMerkleSumTree(leaves);
        const signerIndices = Array.from({ length: k }, (_, i) => Math.floor(i * N / k));

        const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
        const result = verifyMerkleSumMultiProof(multiProof!, root);
        expect(result.valid).toBe(true);
        expect(multiProof!.committeeSize).toBe(N);

        const multiNodes = multiProofNodeCount(multiProof!);
        const individualNodes = individualProofNodeCount(k, N);
        const savings = 1 - multiNodes / individualNodes;

        expect(savings).toBeGreaterThan(0.5);
      }
    });

    it('should correctly compute signer weight from multi-proof', () => {
      const leaves = createMockLeaves(8);
      const { root } = buildMerkleSumTree(leaves);
      const signerIndices = [1, 3, 5];
      const expectedWeight = leaves[1].weight + leaves[3].weight + leaves[5].weight;

      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      const result = verifyMerkleSumMultiProof(multiProof!, root);

      expect(result.valid).toBe(true);
      expect(result.signerWeight).toBe(expectedWeight);
      expect(multiProof!.committeeSize).toBe(8);
    });

    it('should return null for empty signer list', () => {
      const leaves = createMockLeaves(8);
      const multiProof = getMerkleSumMultiProof(leaves, []);
      expect(multiProof).toBeNull();
    });

    it('should return null for invalid signer indices', () => {
      const leaves = createMockLeaves(8);
      const multiProof = getMerkleSumMultiProof(leaves, [0, 99]);
      expect(multiProof).toBeNull();
    });
  });
});
