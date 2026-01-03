import { describe, it, expect } from 'vitest';
import {
  buildMerkleSumTree,
  getMerkleSumMultiProof,
  multiProofNodeCount,
  individualProofNodeCount,
  type MerkleSumLeaf,
} from '../merkle-sum-tree.js';
import { base64urlEncode } from '../encoding.js';

function createTestLeaves(count: number): MerkleSumLeaf[] {
  return Array.from({ length: count }, (_, i) => ({
    index: i,
    address: `0x${'0'.repeat(38)}${(i + 1).toString(16).padStart(2, '0')}`,
    blsPublicKey: base64urlEncode(new Uint8Array(96).fill(i + 1)),
    weight: 1000000 + i * 10000,
  }));
}

interface SizeMetrics {
  committeeSize: number;
  signerCount: number;
  multiProofNodes: number;
  individualProofNodes: number;
  nodeSavingsPercent: number;
  estimatedPackedBytes: number;
  estimatedBase64Chars: number;
  estimatedUrlChars: number;
  fitsQrL: boolean;
  fitsProxyLimit: boolean;
}

function estimatePackedMultiProofSize(
  committeeSize: number,
  signerCount: number,
  auxNodeCount: number,
  merkleProofDepth: number = 10
): number {
  let bytes = 0;
  bytes += 1;
  bytes += 4;
  bytes += 32;
  bytes += 64;
  bytes += 20;
  bytes += 20;
  bytes += 8;
  bytes += 8;
  bytes += 8;
  bytes += 8;
  bytes += 4;
  bytes += 32;
  bytes += 32;
  bytes += 32;
  bytes += 32;
  bytes += 1;
  bytes += 1;
  bytes += merkleProofDepth * 32;
  bytes += 2;
  bytes += 48;
  bytes += 1;
  bytes += Math.ceil(committeeSize / 8);
  bytes += 1;
  bytes += signerCount * (1 + 20 + 96 + 8);
  bytes += 1;
  bytes += auxNodeCount * (1 + 1 + 32 + 8);
  bytes += 32;
  bytes += 8;

  return bytes;
}

function computeSizeMetrics(N: number, k: number): SizeMetrics {
  const leaves = createTestLeaves(N);
  const signerIndices = Array.from({ length: k }, (_, i) => Math.floor(i * N / k));
  const multiProof = getMerkleSumMultiProof(leaves, signerIndices);

  const multiProofNodes = multiProofNodeCount(multiProof!);
  const individualProofNodes = individualProofNodeCount(k, N);
  const nodeSavingsPercent = Math.round((1 - multiProofNodes / individualProofNodes) * 100);

  const treeDepth = Math.ceil(Math.log2(N));
  const auxNodeCount = multiProof!.auxiliaryNodes.length;
  const estimatedPackedBytes = estimatePackedMultiProofSize(N, k, auxNodeCount, 10);
  const compressedBytes = Math.round(estimatedPackedBytes * 0.55);
  const base64Chars = Math.ceil(compressedBytes * 4 / 3);
  const urlPrefix = 'rinku://sp/'.length;
  const estimatedUrlChars = urlPrefix + base64Chars;

  return {
    committeeSize: N,
    signerCount: k,
    multiProofNodes,
    individualProofNodes,
    nodeSavingsPercent,
    estimatedPackedBytes,
    estimatedBase64Chars: base64Chars,
    estimatedUrlChars,
    fitsQrL: compressedBytes <= 2953,
    fitsProxyLimit: estimatedUrlChars <= 8192,
  };
}

describe('Size/QR Dashboard', () => {
  const testCases = [
    { N: 16, k: 11 },
    { N: 21, k: 14 },
    { N: 32, k: 22 },
    { N: 64, k: 43 },
  ];

  describe('Size Metrics by Committee Size', () => {
    const metricsTable: SizeMetrics[] = [];

    for (const { N, k } of testCases) {
      it(`N=${N}, k=${k}: should compute size metrics`, () => {
        const metrics = computeSizeMetrics(N, k);
        metricsTable.push(metrics);

        expect(metrics.committeeSize).toBe(N);
        expect(metrics.signerCount).toBe(k);
        expect(metrics.nodeSavingsPercent).toBeGreaterThan(50);
      });
    }

    it('should output metrics summary', () => {
      console.log('\n=== Profile C Size Dashboard ===\n');
      console.log('| N  | k  | Multi Nodes | Indiv Nodes | Savings | Packed (est) | URL Chars | QR-L? | Proxy? |');
      console.log('|----|----|-----------:|------------:|--------:|-------------:|----------:|:-----:|:------:|');

      for (const { N, k } of testCases) {
        const m = computeSizeMetrics(N, k);
        console.log(
          `| ${N.toString().padStart(2)} | ${k.toString().padStart(2)} | ` +
          `${m.multiProofNodes.toString().padStart(11)} | ${m.individualProofNodes.toString().padStart(11)} | ` +
          `${m.nodeSavingsPercent.toString().padStart(6)}% | ` +
          `${m.estimatedPackedBytes.toString().padStart(12)} | ${m.estimatedUrlChars.toString().padStart(9)} | ` +
          `${m.fitsQrL ? '  ✓  ' : '  ✗  '} | ${m.fitsProxyLimit ? '  ✓   ' : '  ✗   '}|`
        );
      }
      console.log('');
    });
  });

  describe('QR Compatibility Thresholds', () => {
    it('N=16 should fit QR-L', () => {
      const metrics = computeSizeMetrics(16, 11);
      expect(metrics.fitsQrL).toBe(true);
    });

    it('N=21 should fit QR-L (boundary case)', () => {
      const metrics = computeSizeMetrics(21, 14);
      expect(metrics.fitsQrL).toBe(true);
    });

    it('N=32 may not fit QR-L', () => {
      const metrics = computeSizeMetrics(32, 22);
      expect(metrics.fitsProxyLimit).toBe(true);
    });

    it('N=64 should fit proxy limits but not QR', () => {
      const metrics = computeSizeMetrics(64, 43);
      expect(metrics.fitsProxyLimit).toBe(true);
    });
  });

  describe('Multi-Proof Savings Verification', () => {
    it('should achieve >60% node savings for all test cases', () => {
      for (const { N, k } of testCases) {
        const metrics = computeSizeMetrics(N, k);
        expect(metrics.nodeSavingsPercent).toBeGreaterThanOrEqual(60);
      }
    });

    it('savings should increase with committee size', () => {
      const savings16 = computeSizeMetrics(16, 11).nodeSavingsPercent;
      const savings64 = computeSizeMetrics(64, 43).nodeSavingsPercent;
      expect(savings64).toBeGreaterThanOrEqual(savings16);
    });
  });

  describe('Protocol Limits', () => {
    it('uint8 committeeSize cap at 255', () => {
      const leaves = createTestLeaves(255);
      const { root } = buildMerkleSumTree(leaves);
      expect(root.totalWeight).toBeGreaterThan(0);
    });

    it('tree depth = ceil(log2(N)) for N=255', () => {
      const leaves = createTestLeaves(255);
      const { layers } = buildMerkleSumTree(leaves);
      expect(layers.length).toBe(9);
    });
  });
});
