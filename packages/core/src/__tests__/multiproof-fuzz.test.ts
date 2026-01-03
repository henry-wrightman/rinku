import { describe, it, expect } from 'vitest';
import {
  buildMerkleSumTree,
  getMerkleSumProof,
  getMerkleSumMultiProof,
  verifyMerkleSumProof,
  verifyMerkleSumMultiProof,
  type MerkleSumLeaf,
  type MerkleSumMultiProof,
} from '../merkle-sum-tree.js';
import { base64urlEncode } from '../encoding.js';

function seededRandom(seed: number) {
  return () => {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    return seed / 0x7fffffff;
  };
}

function createRandomLeaves(count: number, rng: () => number): MerkleSumLeaf[] {
  return Array.from({ length: count }, (_, i) => ({
    index: i,
    address: `0x${Math.floor(rng() * 0xffffffff).toString(16).padStart(40, '0')}`,
    blsPublicKey: base64urlEncode(new Uint8Array(96).fill(Math.floor(rng() * 256))),
    weight: Math.floor(rng() * 10000000) + 1,
  }));
}

function selectRandomSigners(N: number, rng: () => number): number[] {
  const fraction = 0.5 + rng() * 0.4;
  const k = Math.max(1, Math.floor(N * fraction));
  const indices = Array.from({ length: N }, (_, i) => i);
  for (let i = indices.length - 1; i > 0; i--) {
    const j = Math.floor(rng() * (i + 1));
    [indices[i], indices[j]] = [indices[j], indices[i]];
  }
  return indices.slice(0, k).sort((a, b) => a - b);
}

describe('Multi-Proof Fuzzing', () => {
  describe('Random Committee Verification', () => {
    const iterations = 50;

    for (let seed = 1; seed <= iterations; seed++) {
      it(`seed ${seed}: random committee should verify correctly`, () => {
        const rng = seededRandom(seed);
        const N = Math.floor(rng() * 60) + 4;
        const leaves = createRandomLeaves(N, rng);
        const signerIndices = selectRandomSigners(N, rng);

        const { root } = buildMerkleSumTree(leaves);
        const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

        expect(multiProof).not.toBeNull();
        const result = verifyMerkleSumMultiProof(multiProof!, root);
        expect(result.valid).toBe(true);

        const expectedWeight = signerIndices.reduce((sum, i) => sum + leaves[i].weight, 0);
        expect(result.signerWeight).toBe(expectedWeight);
      });
    }
  });

  describe('Non-Power-of-Two Committees', () => {
    const nonPowerSizes = [3, 5, 7, 9, 11, 13, 17, 19, 21, 23, 31, 33, 63, 65];

    for (const N of nonPowerSizes) {
      it(`N=${N}: non-power-of-two should verify`, () => {
        const rng = seededRandom(N * 100);
        const leaves = createRandomLeaves(N, rng);
        const k = Math.ceil(N * 0.67);
        const signerIndices = Array.from({ length: k }, (_, i) => i);

        const { root } = buildMerkleSumTree(leaves);
        const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

        expect(multiProof).not.toBeNull();
        expect(multiProof!.committeeSize).toBe(N);

        const result = verifyMerkleSumMultiProof(multiProof!, root);
        expect(result.valid).toBe(true);
      });
    }
  });

  describe('Edge Cases', () => {
    it('k=1: single signer', () => {
      const rng = seededRandom(999);
      const leaves = createRandomLeaves(16, rng);
      const signerIndices = [7];

      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

      expect(multiProof).not.toBeNull();
      const result = verifyMerkleSumMultiProof(multiProof!, root);
      expect(result.valid).toBe(true);
      expect(result.signerWeight).toBe(leaves[7].weight);
    });

    it('k=N: all signers', () => {
      const rng = seededRandom(1000);
      const N = 8;
      const leaves = createRandomLeaves(N, rng);
      const signerIndices = Array.from({ length: N }, (_, i) => i);

      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

      expect(multiProof).not.toBeNull();
      expect(multiProof!.auxiliaryNodes.length).toBe(0);

      const result = verifyMerkleSumMultiProof(multiProof!, root);
      expect(result.valid).toBe(true);
    });

    it('k=0: no signers returns null', () => {
      const rng = seededRandom(1001);
      const leaves = createRandomLeaves(8, rng);
      const multiProof = getMerkleSumMultiProof(leaves, []);

      expect(multiProof).toBeNull();
    });

    it('N=2: minimal committee', () => {
      const rng = seededRandom(1002);
      const leaves = createRandomLeaves(2, rng);
      const signerIndices = [0, 1];

      const { root } = buildMerkleSumTree(leaves);
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

      const result = verifyMerkleSumMultiProof(multiProof!, root);
      expect(result.valid).toBe(true);
    });
  });

  describe('Multi-Proof vs Individual Proof Consistency', () => {
    it('should produce same root from both proof types', () => {
      const rng = seededRandom(2000);
      const N = 16;
      const leaves = createRandomLeaves(N, rng);
      const { root } = buildMerkleSumTree(leaves);

      for (let i = 0; i < N; i++) {
        const individualProof = getMerkleSumProof(leaves, i);
        const individualResult = verifyMerkleSumProof(individualProof!, root);
        expect(individualResult.valid).toBe(true);
      }

      const signerIndices = [0, 4, 8, 12];
      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      const multiResult = verifyMerkleSumMultiProof(multiProof!, root);
      expect(multiResult.valid).toBe(true);
    });
  });

  describe('Mutation Testing (MUST Reject)', () => {
    it('should reject when signer weight is tampered', () => {
      const rng = seededRandom(3000);
      const leaves = createRandomLeaves(8, rng);
      const { root } = buildMerkleSumTree(leaves);
      const signerIndices = [0, 2, 5];

      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      const tamperedProof: MerkleSumMultiProof = {
        ...multiProof!,
        leaves: multiProof!.leaves.map((l, i) =>
          i === 0 ? { ...l, weight: l.weight + 1 } : l
        ),
      };

      const result = verifyMerkleSumMultiProof(tamperedProof, root);
      expect(result.valid).toBe(false);
    });

    it('should reject when auxiliary node hash is tampered', () => {
      const rng = seededRandom(3001);
      const leaves = createRandomLeaves(8, rng);
      const { root } = buildMerkleSumTree(leaves);
      const signerIndices = [0, 2, 5];

      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      if (multiProof!.auxiliaryNodes.length > 0) {
        const tamperedProof: MerkleSumMultiProof = {
          ...multiProof!,
          auxiliaryNodes: multiProof!.auxiliaryNodes.map((n, i) =>
            i === 0 ? { ...n, node: { ...n.node, hash: 'tampered' } } : n
          ),
        };

        const result = verifyMerkleSumMultiProof(tamperedProof, root);
        expect(result.valid).toBe(false);
      }
    });

    it('should reject when auxiliary node sumWeight is tampered', () => {
      const rng = seededRandom(3002);
      const leaves = createRandomLeaves(8, rng);
      const { root } = buildMerkleSumTree(leaves);
      const signerIndices = [0, 2, 5];

      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      if (multiProof!.auxiliaryNodes.length > 0) {
        const tamperedProof: MerkleSumMultiProof = {
          ...multiProof!,
          auxiliaryNodes: multiProof!.auxiliaryNodes.map((n, i) =>
            i === 0 ? { ...n, node: { ...n.node, sumWeight: n.node.sumWeight + 1 } } : n
          ),
        };

        const result = verifyMerkleSumMultiProof(tamperedProof, root);
        expect(result.valid).toBe(false);
      }
    });

    it('should reject when leaf address is tampered', () => {
      const rng = seededRandom(3003);
      const leaves = createRandomLeaves(8, rng);
      const { root } = buildMerkleSumTree(leaves);
      const signerIndices = [0, 2, 5];

      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      const tamperedProof: MerkleSumMultiProof = {
        ...multiProof!,
        leaves: multiProof!.leaves.map((l, i) =>
          i === 0 ? { ...l, address: 'tampered_address' } : l
        ),
      };

      const result = verifyMerkleSumMultiProof(tamperedProof, root);
      expect(result.valid).toBe(false);
    });

    it('should reject wrong root totalWeight even if hash matches', () => {
      const rng = seededRandom(3004);
      const leaves = createRandomLeaves(8, rng);
      const { root } = buildMerkleSumTree(leaves);
      const signerIndices = [0, 2, 5];

      const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
      const wrongRoot = { hash: root.hash, totalWeight: root.totalWeight + 1 };

      const result = verifyMerkleSumMultiProof(multiProof!, wrongRoot);
      expect(result.valid).toBe(false);
    });
  });

  describe('Canonical Ordering Under Fuzzing', () => {
    it('auxiliary nodes should always be sorted (level, index)', () => {
      for (let seed = 1; seed <= 20; seed++) {
        const rng = seededRandom(seed * 1000);
        const N = Math.floor(rng() * 30) + 8;
        const leaves = createRandomLeaves(N, rng);
        const signerIndices = selectRandomSigners(N, rng);

        const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
        if (multiProof && multiProof.auxiliaryNodes.length > 1) {
          for (let i = 1; i < multiProof.auxiliaryNodes.length; i++) {
            const prev = multiProof.auxiliaryNodes[i - 1];
            const curr = multiProof.auxiliaryNodes[i];
            const isOrdered = prev.level < curr.level ||
              (prev.level === curr.level && prev.index < curr.index);
            expect(isOrdered).toBe(true);
          }
        }
      }
    });

    it('no duplicate (level, index) positions in auxiliary nodes', () => {
      for (let seed = 1; seed <= 20; seed++) {
        const rng = seededRandom(seed * 2000);
        const N = Math.floor(rng() * 30) + 8;
        const leaves = createRandomLeaves(N, rng);
        const signerIndices = selectRandomSigners(N, rng);

        const multiProof = getMerkleSumMultiProof(leaves, signerIndices);
        if (multiProof) {
          const seen = new Set<string>();
          for (const node of multiProof.auxiliaryNodes) {
            const key = `${node.level}:${node.index}`;
            expect(seen.has(key)).toBe(false);
            seen.add(key);
          }
        }
      }
    });
  });
});
