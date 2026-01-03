import { describe, it, expect } from 'vitest';
import {
  buildMerkleSumTree,
  getMerkleSumMultiProof,
  verifyMerkleSumMultiProof,
  EMPTY_NODE,
  type MerkleSumLeaf,
  type MerkleSumRoot,
  type MerkleSumMultiProof,
} from '../merkle-sum-tree.js';
import { base64urlEncode } from '../encoding.js';

function createTestLeaves(count: number, baseWeight: number = 1000000): MerkleSumLeaf[] {
  return Array.from({ length: count }, (_, i) => ({
    index: i,
    address: `0x${'0'.repeat(38)}${(i + 1).toString(16).padStart(2, '0')}`,
    blsPublicKey: base64urlEncode(new Uint8Array(96).fill(i + 1)),
    weight: baseWeight
  }));
}

describe('Appendix G: Normative Test Vectors', () => {
  describe('G.1: MerkleSumTree Test Vector (N=4)', () => {
    const leaves = [
      { index: 0, address: '0x' + '0'.repeat(38) + '01', blsPublicKey: base64urlEncode(new Uint8Array(96).fill(0x01)), weight: 1000000 },
      { index: 1, address: '0x' + '0'.repeat(38) + '02', blsPublicKey: base64urlEncode(new Uint8Array(96).fill(0x02)), weight: 2000000 },
      { index: 2, address: '0x' + '0'.repeat(38) + '03', blsPublicKey: base64urlEncode(new Uint8Array(96).fill(0x03)), weight: 1500000 },
      { index: 3, address: '0x' + '0'.repeat(38) + '04', blsPublicKey: base64urlEncode(new Uint8Array(96).fill(0x04)), weight: 500000 },
    ];

    it('should compute correct totalWeight for N=4 committee', () => {
      const { root } = buildMerkleSumTree(leaves);
      expect(root.totalWeight).toBe(5000000);
    });

    it('should produce deterministic root hash', () => {
      const { root: root1 } = buildMerkleSumTree(leaves);
      const { root: root2 } = buildMerkleSumTree(leaves);
      expect(root1.hash).toBe(root2.hash);
      expect(root1.totalWeight).toBe(root2.totalWeight);
    });

    it('should produce different hash for different weights', () => {
      const modifiedLeaves = [...leaves];
      modifiedLeaves[0] = { ...modifiedLeaves[0], weight: 999999 };
      const { root: original } = buildMerkleSumTree(leaves);
      const { root: modified } = buildMerkleSumTree(modifiedLeaves);
      expect(original.hash).not.toBe(modified.hash);
    });
  });

  describe('G.2: Multi-Proof Test Vector (k=3 of N=4)', () => {
    const leaves = [
      { index: 0, address: '0x' + '0'.repeat(38) + '01', blsPublicKey: base64urlEncode(new Uint8Array(96).fill(0x01)), weight: 1000000 },
      { index: 1, address: '0x' + '0'.repeat(38) + '02', blsPublicKey: base64urlEncode(new Uint8Array(96).fill(0x02)), weight: 2000000 },
      { index: 2, address: '0x' + '0'.repeat(38) + '03', blsPublicKey: base64urlEncode(new Uint8Array(96).fill(0x03)), weight: 1500000 },
      { index: 3, address: '0x' + '0'.repeat(38) + '04', blsPublicKey: base64urlEncode(new Uint8Array(96).fill(0x04)), weight: 500000 },
    ];
    const signerIndices = [0, 1, 3];

    it('should verify multi-proof with k=3 signers', () => {
      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      expect(multiProof).not.toBeNull();

      const result = verifyMerkleSumMultiProof(multiProof!, root);
      expect(result.valid).toBe(true);
    });

    it('should compute correct signer weight (70%)', () => {
      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      const result = verifyMerkleSumMultiProof(multiProof!, root);

      expect(result.signerWeight).toBe(3500000);
      expect(root.totalWeight).toBe(5000000);
      const ratio = result.signerWeight / root.totalWeight;
      expect(ratio).toBe(0.7);
    });

    it('should pass 67% quorum threshold', () => {
      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      const result = verifyMerkleSumMultiProof(multiProof!, root);

      const quorumThresholdBps = 6700;
      const ratio = result.signerWeight / root.totalWeight;
      expect(ratio * 10000).toBeGreaterThanOrEqual(quorumThresholdBps);
    });

    it('should include auxiliary node for non-signer (index 2)', () => {
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      expect(multiProof!.auxiliaryNodes.length).toBeGreaterThan(0);
    });
  });

  describe('G.5: Non-Power-of-Two Test Vector (N=5)', () => {
    const leaves = createTestLeaves(5);

    it('should compute correct totalWeight for N=5 committee', () => {
      const { root } = buildMerkleSumTree(leaves);
      expect(root.totalWeight).toBe(5000000);
    });

    it('should verify multi-proof with EMPTY_NODE padding', () => {
      const { root } = buildMerkleSumTree(leaves);
      const signerIndices = [0, 1, 2, 4];
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

      const result = verifyMerkleSumMultiProof(multiProof!, root);
      expect(result.valid).toBe(true);
      expect(result.signerWeight).toBe(4000000);
    });

    it('should handle tree depth = ceil(log2(5)) = 3', () => {
      const { layers } = buildMerkleSumTree(leaves);
      expect(layers.length).toBe(4);
    });

    it('should verify EMPTY_NODE is used for padding', () => {
      expect(EMPTY_NODE.sumWeight).toBe(0);
      expect(EMPTY_NODE.hash).toBeTruthy();
    });
  });

  describe('G.6: Negative Test Vectors (MUST Reject)', () => {
    const leaves = createTestLeaves(4);
    const signerIndices = [0, 1, 3];

    it('1. MUST reject: root hash mismatch', () => {
      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      const wrongRoot: MerkleSumRoot = { hash: 'wronghash', totalWeight: root.totalWeight };

      const result = verifyMerkleSumMultiProof(multiProof!, wrongRoot);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('hash mismatch'))).toBe(true);
    });

    it('2. MUST reject: root totalWeight mismatch', () => {
      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      const wrongRoot: MerkleSumRoot = { hash: root.hash, totalWeight: 999 };

      const result = verifyMerkleSumMultiProof(multiProof!, wrongRoot);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('weight mismatch'))).toBe(true);
    });

    it('3. Quorum check: verify signerWeight/totalWeight ratio', () => {
      const lowWeightLeaves = [
        { ...leaves[0], weight: 3000000 },
        { ...leaves[1], weight: 2000000 },
        { ...leaves[2], weight: 1500000 },
        { ...leaves[3], weight: 3500000 },
      ];
      const { root } = buildMerkleSumTree(lowWeightLeaves);
      const multiProof = getMerkleSumMultiProof(lowWeightLeaves, [0, 1]);

      const result = verifyMerkleSumMultiProof(multiProof!, root);
      expect(result.valid).toBe(true);

      const ratio = result.signerWeight / root.totalWeight;
      expect(ratio).toBeLessThan(0.67);
      const quorumThresholdBps = 6700;
      const meetsQuorum = (ratio * 10000) >= quorumThresholdBps;
      expect(meetsQuorum).toBe(false);
    });

    it('4. MUST reject: empty signer set (verifier rejects)', () => {
      const { root } = buildMerkleSumTree(leaves);
      const emptyProof: MerkleSumMultiProof = {
        committeeSize: 4,
        leaves: [],
        auxiliaryNodes: []
      };

      const result = verifyMerkleSumMultiProof(emptyProof, root);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('No leaves'))).toBe(true);
    });

    it('5. MUST reject: invalid signer index (builder returns null)', () => {
      const multiProof = getMerkleSumMultiProof(leaves, [0, 1, 99]);
      expect(multiProof).toBeNull();
    });

    it('6. MUST reject: tampered leaf weight causes root mismatch', () => {
      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

      const tamperedProof: MerkleSumMultiProof = {
        ...multiProof!,
        leaves: multiProof!.leaves.map((l, i) =>
          i === 0 ? { ...l, weight: l.weight + 1 } : l
        )
      };

      const result = verifyMerkleSumMultiProof(tamperedProof, root);
      expect(result.valid).toBe(false);
    });

    it('7. MUST reject: tampered auxiliary node hash', () => {
      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

      if (multiProof!.auxiliaryNodes.length > 0) {
        const tamperedProof: MerkleSumMultiProof = {
          ...multiProof!,
          auxiliaryNodes: multiProof!.auxiliaryNodes.map((n, i) =>
            i === 0 ? { ...n, node: { ...n.node, hash: 'tampered_hash' } } : n
          )
        };

        const result = verifyMerkleSumMultiProof(tamperedProof, root);
        expect(result.valid).toBe(false);
      }
    });

    it('8. MUST reject: tampered auxiliary node sumWeight', () => {
      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

      if (multiProof!.auxiliaryNodes.length > 0) {
        const tamperedProof: MerkleSumMultiProof = {
          ...multiProof!,
          auxiliaryNodes: multiProof!.auxiliaryNodes.map((n, i) =>
            i === 0 ? { ...n, node: { ...n.node, sumWeight: n.node.sumWeight + 1 } } : n
          )
        };

        const result = verifyMerkleSumMultiProof(tamperedProof, root);
        expect(result.valid).toBe(false);
      }
    });

    it('9. MUST reject: tampered leaf address', () => {
      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

      const tamperedProof: MerkleSumMultiProof = {
        ...multiProof!,
        leaves: multiProof!.leaves.map((l, i) =>
          i === 0 ? { ...l, address: 'tampered_address_here' } : l
        )
      };

      const result = verifyMerkleSumMultiProof(tamperedProof, root);
      expect(result.valid).toBe(false);
    });

    it('10. MUST reject: wrong committeeSize causes depth mismatch', () => {
      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

      const tamperedProof: MerkleSumMultiProof = {
        ...multiProof!,
        committeeSize: 16
      };

      const result = verifyMerkleSumMultiProof(tamperedProof, root);
      expect(result.valid).toBe(false);
    });
  });

  describe('Canonical Constraints', () => {
    it('should produce consistent auxiliary nodes across calls', () => {
      const leaves = createTestLeaves(8);
      const signerIndices = [0, 2, 5, 7];

      const proof1 = getMerkleSumMultiProof(leaves, signerIndices);
      const proof2 = getMerkleSumMultiProof(leaves, signerIndices);

      expect(proof1!.auxiliaryNodes.length).toBe(proof2!.auxiliaryNodes.length);
      for (let i = 0; i < proof1!.auxiliaryNodes.length; i++) {
        expect(proof1!.auxiliaryNodes[i].level).toBe(proof2!.auxiliaryNodes[i].level);
        expect(proof1!.auxiliaryNodes[i].index).toBe(proof2!.auxiliaryNodes[i].index);
      }
    });

    it('should have auxiliary nodes sorted by (level, index)', () => {
      const leaves = createTestLeaves(16);
      const signerIndices = [0, 4, 8, 12];
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

      for (let i = 1; i < multiProof!.auxiliaryNodes.length; i++) {
        const prev = multiProof!.auxiliaryNodes[i - 1];
        const curr = multiProof!.auxiliaryNodes[i];
        const prevKey = prev.level * 1000 + prev.index;
        const currKey = curr.level * 1000 + curr.index;
        expect(currKey).toBeGreaterThan(prevKey);
      }
    });

    it('should have no duplicate auxiliary node positions', () => {
      const leaves = createTestLeaves(16);
      const signerIndices = [1, 3, 5, 7, 9, 11];
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

      const positions = new Set<string>();
      for (const node of multiProof!.auxiliaryNodes) {
        const key = `${node.level}:${node.index}`;
        expect(positions.has(key)).toBe(false);
        positions.add(key);
      }
    });

    it('should have no overlap between auxiliary nodes and signer leaves at level 0', () => {
      const leaves = createTestLeaves(8);
      const signerIndices = [0, 2, 5];
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

      const signerIndexSet = new Set(signerIndices);
      for (const node of multiProof!.auxiliaryNodes) {
        if (node.level === 0) {
          expect(signerIndexSet.has(node.index)).toBe(false);
        }
      }
    });
  });
});
