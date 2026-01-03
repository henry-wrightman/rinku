import { describe, it, expect } from 'vitest';

interface MembershipProof {
  leaf: {
    index: number;
    address: string;
    blsPublicKey: string;
    weight: number;
  };
  siblings: Array<{ hash: string; sumWeight: number }>;
  pathBits: boolean[];
}

interface ProfileCProof {
  version: number;
  tx: {
    from: string;
    to: string;
    amount: number;
    fee: number;
    nonce: number;
    ts: number;
    sig: string;
  };
  txHash: string;
  checkpoint: {
    id: string;
    height: number;
    txMerkleRoot: string;
    stateRoot: string;
    receiptRoot: string;
    tipCount: number;
  };
  merkleProof: {
    hashes: string[];
    index: number;
  };
  blsAggSig: string;
  signerBitmap: string;
  validatorSumTreeRoot: {
    hash: string;
    totalWeight: number;
  };
  signerMembershipProofs: MembershipProof[];
}

function createMockProfileCProof(committeeSize: number, signerCount: number): ProfileCProof {
  const treeDepth = Math.ceil(Math.log2(committeeSize));
  
  const signerMembershipProofs: MembershipProof[] = [];
  for (let i = 0; i < signerCount; i++) {
    const siblings: Array<{ hash: string; sumWeight: number }> = [];
    for (let level = 0; level < treeDepth; level++) {
      siblings.push({
        hash: 'a'.repeat(64),
        sumWeight: 1000000 + level * 100
      });
    }
    
    signerMembershipProofs.push({
      leaf: {
        index: i,
        address: 'b'.repeat(40),
        blsPublicKey: 'c'.repeat(128),
        weight: 100000
      },
      siblings,
      pathBits: new Array(treeDepth).fill(false).map((_, i) => i % 2 === 0)
    });
  }

  return {
    version: 4,
    tx: {
      from: 'd'.repeat(40),
      to: 'e'.repeat(40),
      amount: 1000,
      fee: 1,
      nonce: 42,
      ts: 1704067200000,
      sig: 'f'.repeat(88)
    },
    txHash: 'g'.repeat(64),
    checkpoint: {
      id: 'h'.repeat(64),
      height: 1000,
      txMerkleRoot: 'i'.repeat(64),
      stateRoot: 'j'.repeat(64),
      receiptRoot: 'k'.repeat(64),
      tipCount: 150
    },
    merkleProof: {
      hashes: new Array(10).fill('l'.repeat(64)),
      index: 42
    },
    blsAggSig: 'm'.repeat(64),
    signerBitmap: 'n'.repeat(Math.ceil(committeeSize / 4)),
    validatorSumTreeRoot: {
      hash: 'o'.repeat(64),
      totalWeight: 10000000
    },
    signerMembershipProofs
  };
}

function estimatePackedSize(proof: ProfileCProof): number {
  const treeDepth = proof.signerMembershipProofs[0]?.siblings.length || 0;
  const signerCount = proof.signerMembershipProofs.length;
  
  let size = 0;
  size += 1;
  size += 32;
  size += 64;
  size += 4;
  size += 32 * 3;
  size += 4;
  size += 1 + proof.merkleProof.hashes.length * 32 + 4;
  size += 48;
  size += 1 + Math.ceil(64 / 8);
  size += 32 + 8;
  for (let i = 0; i < signerCount; i++) {
    size += 4;
    size += 20;
    size += 96;
    size += 8;
    size += 1;
    size += treeDepth * 40;
    size += Math.ceil(treeDepth / 8);
  }
  
  return size;
}

describe('Profile C Size Analysis', () => {
  it('should measure JSON encoding sizes for various committee configurations', () => {
    console.log('\n=== PROFILE C SIZE ANALYSIS (JSON ENCODING) ===');
    console.log('Committee | Signers | Tree Depth | JSON Size | Est. DEFLATE | QR-L Fit?');
    console.log('----------|---------|------------|-----------|--------------|----------');
    
    const configs = [
      { N: 16, k: 11 },
      { N: 21, k: 14 },
      { N: 32, k: 22 },
      { N: 64, k: 43 }
    ];
    
    for (const { N, k } of configs) {
      const proof = createMockProfileCProof(N, k);
      const jsonStr = JSON.stringify(proof);
      const jsonSize = jsonStr.length;
      const estimatedDeflate = Math.round(jsonSize * 0.55);
      const qrFit = estimatedDeflate <= 2953 ? '✓' : '✗';
      const treeDepth = Math.ceil(Math.log2(N));
      
      console.log(
        `${N.toString().padStart(9)} | ${k.toString().padStart(7)} | ${treeDepth.toString().padStart(10)} | ${jsonSize.toString().padStart(9)} | ${estimatedDeflate.toString().padStart(12)} | ${qrFit.padStart(8)}`
      );
    }
  });

  it('should measure packed binary encoding sizes', () => {
    console.log('\n=== PROFILE C SIZE ANALYSIS (PACKED BINARY) ===');
    console.log('Committee | Signers | Tree Depth | Packed Size | Est. DEFLATE | QR-L Fit?');
    console.log('----------|---------|------------|-------------|--------------|----------');
    
    const configs = [
      { N: 16, k: 11 },
      { N: 21, k: 14 },
      { N: 32, k: 22 },
      { N: 64, k: 43 }
    ];
    
    for (const { N, k } of configs) {
      const proof = createMockProfileCProof(N, k);
      const packedSize = estimatePackedSize(proof);
      const estimatedDeflate = Math.round(packedSize * 0.65);
      const qrFit = estimatedDeflate <= 2953 ? '✓' : '✗';
      const treeDepth = Math.ceil(Math.log2(N));
      
      console.log(
        `${N.toString().padStart(9)} | ${k.toString().padStart(7)} | ${treeDepth.toString().padStart(10)} | ${packedSize.toString().padStart(11)} | ${estimatedDeflate.toString().padStart(12)} | ${qrFit.padStart(8)}`
      );
    }
  });

  it('should break down component sizes', () => {
    console.log('\n=== PROFILE C COMPONENT SIZE BREAKDOWN ===');
    
    const proof = createMockProfileCProof(21, 14);
    const json = JSON.stringify(proof);
    
    console.log(`Total JSON size: ${json.length} bytes\n`);
    console.log('Component breakdown (JSON):');
    console.log(`  tx: ${JSON.stringify(proof.tx).length} bytes`);
    console.log(`  txHash: ${proof.txHash.length + 12} bytes`);
    console.log(`  checkpoint: ${JSON.stringify(proof.checkpoint).length} bytes`);
    console.log(`  merkleProof: ${JSON.stringify(proof.merkleProof).length} bytes`);
    console.log(`  blsAggSig: ${proof.blsAggSig.length + 14} bytes`);
    console.log(`  signerBitmap: ${proof.signerBitmap.length + 17} bytes`);
    console.log(`  validatorSumTreeRoot: ${JSON.stringify(proof.validatorSumTreeRoot).length} bytes`);
    console.log(`  signerMembershipProofs: ${JSON.stringify(proof.signerMembershipProofs).length} bytes`);
    
    const avgProofSize = JSON.stringify(proof.signerMembershipProofs).length / proof.signerMembershipProofs.length;
    console.log(`\n  Average per-signer proof: ${Math.round(avgProofSize)} bytes`);
  });

  it('should verify N=16 k=11 fits QR with packed encoding', () => {
    const proof = createMockProfileCProof(16, 11);
    const packedSize = estimatePackedSize(proof);
    const estimatedDeflate = Math.round(packedSize * 0.65);
    
    console.log(`\n=== QR COMPATIBILITY CHECK (N=16, k=11) ===`);
    console.log(`Packed binary size: ${packedSize} bytes`);
    console.log(`Estimated DEFLATE: ${estimatedDeflate} bytes`);
    console.log(`QR-L capacity: 2953 bytes`);
    console.log(`Margin: ${2953 - estimatedDeflate} bytes`);
    console.log(`Result: ${estimatedDeflate <= 2953 ? 'FITS ✓' : 'TOO LARGE ✗'}`);
    
    expect(estimatedDeflate).toBeLessThan(2953);
  });

  it('should verify N=21 k=14 requires URL sharing (too large for QR)', () => {
    const proof = createMockProfileCProof(21, 14);
    const packedSize = estimatePackedSize(proof);
    const estimatedDeflate = Math.round(packedSize * 0.65);
    
    console.log(`\n=== N=21 k=14 SIZE CHECK ===`);
    console.log(`Packed + DEFLATE: ${estimatedDeflate} bytes`);
    console.log(`Exceeds QR-L by: ${estimatedDeflate - 2953} bytes`);
    console.log(`Result: URL sharing required`);
    
    expect(estimatedDeflate).toBeGreaterThan(2953);
    expect(estimatedDeflate).toBeLessThan(65536);
  });

  it('should verify N=32 k=22 requires URL sharing', () => {
    const proof = createMockProfileCProof(32, 22);
    const packedSize = estimatePackedSize(proof);
    const estimatedDeflate = Math.round(packedSize * 0.65);
    
    console.log(`\n=== URL SHARING CHECK (N=32, k=22) ===`);
    console.log(`Packed binary size: ${packedSize} bytes`);
    console.log(`Estimated DEFLATE: ${estimatedDeflate} bytes`);
    console.log(`QR-L capacity: 2953 bytes`);
    console.log(`Browser URL limit: 65536 bytes`);
    console.log(`Result: ${estimatedDeflate <= 65536 ? 'URL SHAREABLE ✓' : 'TOO LARGE ✗'}`);
    
    expect(estimatedDeflate).toBeGreaterThan(2953);
    expect(estimatedDeflate).toBeLessThan(65536);
  });
});
