import { describe, it } from 'vitest';
import { 
  createSelfCrawlableURL,
  createTransactionURL
} from '../encoding.js';
import type { Transaction, SelfCrawlableBundle, CheckpointAnchor } from '../types.js';

function randomHex(length: number): string {
  const chars = '0123456789abcdef';
  let result = '';
  for (let i = 0; i < length; i++) {
    result += chars[Math.floor(Math.random() * 16)];
  }
  return result;
}

function createRealisticTransaction(nonce: number): Transaction {
  return {
    from: randomHex(40),
    to: randomHex(40),
    amount: Math.floor(Math.random() * 10000) / 100,
    fee: 0.001 + Math.random() * 0.01,
    nonce,
    tipUrls: [],
    ts: Date.now() - Math.floor(Math.random() * 86400000),
    sig: 'MEUCIQDKZokqnCjrRtw5Y2mG0L7UGjC5FwXxVMXaWj9AaGJaBwIgZbHkVj3vNbjWno2TLpZ0tXcKxWjWxPq2RmN8kU0Y1qE'
  };
}

function createRealisticCheckpointAnchor(): CheckpointAnchor {
  return {
    checkpointId: randomHex(64),
    height: Math.floor(Math.random() * 10000),
    merkleRoot: randomHex(64),
    signatureCount: 5
  };
}

function buildRealisticBundle(depth: number, branchingFactor: number = 1): SelfCrawlableBundle {
  let nonce = 0;
  
  function buildRecursive(currentDepth: number): SelfCrawlableBundle {
    const tx = createRealisticTransaction(nonce++);
    const hash = randomHex(64);
    
    if (currentDepth >= depth) {
      return {
        tx,
        hash,
        parents: [],
        checkpointAnchor: createRealisticCheckpointAnchor()
      };
    }
    
    const parents: SelfCrawlableBundle[] = [];
    for (let i = 0; i < branchingFactor && i < 2; i++) {
      parents.push(buildRecursive(currentDepth + 1));
    }
    
    return {
      tx,
      hash,
      parents
    };
  }
  
  return buildRecursive(0);
}

function countTransactions(bundle: SelfCrawlableBundle): number {
  let count = 1;
  for (const parent of bundle.parents) {
    count += countTransactions(parent);
  }
  return count;
}

describe('Realistic Proof Bundle Size Analysis', () => {
  it('should measure realistic proof sizes with high-entropy data', () => {
    console.log('\n=== REALISTIC PROOF BUNDLE SIZES (HIGH-ENTROPY DATA) ===');
    console.log('Uses random 64-char hex hashes, random addresses, real ECDSA-length signatures\n');
    
    console.log('Fields included per transaction:');
    console.log('  - from: 40-char random hex');
    console.log('  - to: 40-char random hex');
    console.log('  - amount/fee/nonce/ts: numeric values');
    console.log('  - sig: 88-char ECDSA signature');
    console.log('  - hash: 64-char random hex\n');
    
    console.log('Fields included per checkpoint anchor:');
    console.log('  - checkpointId: 64-char random hex');
    console.log('  - merkleRoot: 64-char random hex');
    console.log('  - height: number');
    console.log('  - signatureCount: number (NOT full signatures array)\n');
    
    console.log('Depth | TXs | JSON (bytes) | Encoded (chars) | URL Path (chars)');
    console.log('------|-----|--------------|-----------------|------------------');
    
    for (const depth of [0, 1, 2, 3, 5, 10]) {
      const bundle = buildRealisticBundle(depth, 1);
      const txCount = countTransactions(bundle);
      const jsonSize = JSON.stringify(bundle).length;
      const urlResult = createSelfCrawlableURL(bundle);
      const encodedSize = urlResult.payload.length;
      const urlSize = urlResult.path.length;
      
      console.log(`${depth.toString().padStart(5)} | ${txCount.toString().padStart(3)} | ${jsonSize.toString().padStart(12)} | ${encodedSize.toString().padStart(15)} | ${urlSize.toString().padStart(16)}`);
    }
  });

  it('should measure DAG proofs with branching and high-entropy data', () => {
    console.log('\n=== REALISTIC DAG PROOFS (2 parents per tx) ===');
    console.log('Depth | TXs | JSON (bytes) | Encoded (chars) | URL Path (chars)');
    console.log('------|-----|--------------|-----------------|------------------');
    
    for (const depth of [0, 1, 2, 3, 4]) {
      const bundle = buildRealisticBundle(depth, 2);
      const txCount = countTransactions(bundle);
      const jsonSize = JSON.stringify(bundle).length;
      const urlResult = createSelfCrawlableURL(bundle);
      const encodedSize = urlResult.payload.length;
      const urlSize = urlResult.path.length;
      
      console.log(`${depth.toString().padStart(5)} | ${txCount.toString().padStart(3)} | ${jsonSize.toString().padStart(12)} | ${encodedSize.toString().padStart(15)} | ${urlSize.toString().padStart(16)}`);
    }
  });

  it('should analyze QR code compatibility with byte mode', () => {
    console.log('\n=== QR CODE COMPATIBILITY (BYTE MODE) ===');
    console.log('Base64url uses characters outside alphanumeric set, requiring byte mode\n');
    
    const qrLimits = {
      'QR L (7% EC)': 2953,
      'QR M (15% EC)': 2331,
      'QR Q (25% EC)': 1663,
      'QR H (30% EC)': 1273
    };
    
    const testCases = [
      { name: 'Single tx', bundle: buildRealisticBundle(0, 1) },
      { name: '1 parent', bundle: buildRealisticBundle(1, 1) },
      { name: '2 parents deep', bundle: buildRealisticBundle(2, 1) },
      { name: '3 parents deep', bundle: buildRealisticBundle(3, 1) },
      { name: '5 parents deep', bundle: buildRealisticBundle(5, 1) }
    ];
    
    console.log('Proof Type      | URL Size | QR-L | QR-M | QR-Q | QR-H');
    console.log('----------------|----------|------|------|------|------');
    
    for (const tc of testCases) {
      const urlResult = createSelfCrawlableURL(tc.bundle);
      const size = urlResult.path.length;
      
      const l = size <= 2953 ? 'OK' : 'NO';
      const m = size <= 2331 ? 'OK' : 'NO';
      const q = size <= 1663 ? 'OK' : 'NO';
      const h = size <= 1273 ? 'OK' : 'NO';
      
      console.log(`${tc.name.padEnd(15)} | ${size.toString().padStart(8)} | ${l.padStart(4)} | ${m.padStart(4)} | ${q.padStart(4)} | ${h.padStart(4)}`);
    }
    
    console.log('\nQR Code capacity (byte mode, Version 40):');
    console.log('  L (7% error correction):  2,953 bytes');
    console.log('  M (15% error correction): 2,331 bytes');
    console.log('  Q (25% error correction): 1,663 bytes');
    console.log('  H (30% error correction): 1,273 bytes');
  });

  it('should provide honest assessment', () => {
    console.log('\n=== HONEST ASSESSMENT ===\n');
    
    const single = buildRealisticBundle(0, 1);
    const depth2 = buildRealisticBundle(2, 1);
    const depth5 = buildRealisticBundle(5, 1);
    
    const singleUrl = createSelfCrawlableURL(single);
    const depth2Url = createSelfCrawlableURL(depth2);
    const depth5Url = createSelfCrawlableURL(depth5);
    
    console.log('What IS included in proof bundles:');
    console.log('  - Full transaction data (from, to, amount, fee, nonce, ts, sig)');
    console.log('  - Transaction hash (64 chars)');
    console.log('  - Checkpoint anchor (id, merkleRoot, height, signatureCount)');
    console.log('  - Recursive parent bundles\n');
    
    console.log('What is NOT included:');
    console.log('  - Full validator signatures array (only signatureCount)');
    console.log('  - Full txHashes array from checkpoint');
    console.log('  - Merkle proof paths (could add ~100-500 bytes)\n');
    
    console.log('Realistic sizes with high-entropy data:');
    console.log(`  Single tx proof: ${singleUrl.path.length} chars`);
    console.log(`  2-depth proof: ${depth2Url.path.length} chars`);
    console.log(`  5-depth proof: ${depth5Url.path.length} chars\n`);
    
    console.log('QR Code compatibility (with M-level error correction):');
    console.log(`  Single tx: ${singleUrl.path.length <= 2331 ? 'FITS' : 'TOO LARGE'}`);
    console.log(`  2-depth: ${depth2Url.path.length <= 2331 ? 'FITS' : 'TOO LARGE'}`);
    console.log(`  5-depth: ${depth5Url.path.length <= 2331 ? 'FITS' : 'TOO LARGE'}`);
  });
});
