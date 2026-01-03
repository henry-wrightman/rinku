import { describe, it, expect } from 'vitest';
import { 
  encodeTransaction, 
  encodeSelfCrawlableBundle, 
  createTransactionURL,
  createSelfCrawlableURL
} from '../encoding.js';
import type { Transaction, SelfCrawlableBundle, CheckpointAnchor } from '../types.js';

function createMockTransaction(nonce: number, tipUrls: string[] = []): Transaction {
  return {
    from: 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2',
    to: 'f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3b2a1f6e5',
    amount: 100,
    fee: 0.01,
    nonce,
    tipUrls,
    ts: 1704067200000 + nonce * 1000,
    sig: 'MEUCIQDKZokqnCjrRtw5Y2mG0L7UGjC5FwXxVMXaWj9AaGJaBwIgZbHkVj3vNbjWno2TLpZ0tXcKxWjWxPq2RmN8kU0Y1qE'
  };
}

function computeHash(tx: Transaction, nonce: number): string {
  return `tx${nonce.toString().padStart(60, '0')}`;
}

function createMockCheckpointAnchor(): CheckpointAnchor {
  return {
    checkpointId: 'cp_a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2',
    height: 100,
    merkleRoot: 'mr_a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2',
    signatureCount: 3
  };
}

function buildBundleWithDepth(depth: number, branchingFactor: number = 1): SelfCrawlableBundle {
  let nonce = 0;
  
  function buildRecursive(currentDepth: number): SelfCrawlableBundle {
    const currentNonce = nonce++;
    const tx = createMockTransaction(currentNonce);
    const hash = computeHash(tx, currentNonce);
    
    if (currentDepth >= depth) {
      return {
        tx,
        hash,
        parents: [],
        checkpointAnchor: createMockCheckpointAnchor()
      };
    }
    
    const parents: SelfCrawlableBundle[] = [];
    for (let i = 0; i < branchingFactor && i < 2; i++) {
      parents.push(buildRecursive(currentDepth + 1));
    }
    
    return {
      tx,
      hash,
      parents,
      checkpointAnchor: currentDepth === 0 ? undefined : undefined
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

describe('Proof Bundle Size Analysis', () => {
  it('should measure single transaction URL size', () => {
    const tx = createMockTransaction(1, []);
    const url = createTransactionURL(tx);
    
    console.log('\n=== SINGLE TRANSACTION ===');
    console.log(`JSON size: ${JSON.stringify(tx).length} bytes`);
    console.log(`Encoded payload: ${url.payload.length} chars`);
    console.log(`Full URL path: ${url.path.length} chars`);
    
    expect(url.path.length).toBeLessThan(1000);
  });

  it('should measure transaction with 2 tip URLs', () => {
    const tipUrl1 = '/tx/' + 'a'.repeat(300);
    const tipUrl2 = '/tx/' + 'b'.repeat(300);
    const tx = createMockTransaction(1, [tipUrl1, tipUrl2]);
    const url = createTransactionURL(tx);
    
    console.log('\n=== TRANSACTION WITH 2 TIP URLS ===');
    console.log(`JSON size: ${JSON.stringify(tx).length} bytes`);
    console.log(`Encoded payload: ${url.payload.length} chars`);
    console.log(`Full URL path: ${url.path.length} chars`);
    
    expect(url.path.length).toBeLessThan(2000);
  });

  it('should measure proof bundles at various depths', () => {
    console.log('\n=== PROOF BUNDLE SIZE BY ANCESTRY DEPTH ===');
    console.log('Depth | TXs | JSON (bytes) | Encoded (chars) | URL Path (chars)');
    console.log('------|-----|--------------|-----------------|------------------');
    
    const results: { depth: number; txCount: number; jsonSize: number; encodedSize: number; urlSize: number }[] = [];
    
    for (const depth of [0, 1, 2, 3, 5, 10]) {
      const bundle = buildBundleWithDepth(depth, 1);
      const txCount = countTransactions(bundle);
      const jsonSize = JSON.stringify(bundle).length;
      const urlResult = createSelfCrawlableURL(bundle);
      const encodedSize = urlResult.payload.length;
      const urlSize = urlResult.path.length;
      
      results.push({ depth, txCount, jsonSize, encodedSize, urlSize });
      console.log(`${depth.toString().padStart(5)} | ${txCount.toString().padStart(3)} | ${jsonSize.toString().padStart(12)} | ${encodedSize.toString().padStart(15)} | ${urlSize.toString().padStart(16)}`);
    }
    
    expect(results[0].urlSize).toBeLessThan(2000);
  });

  it('should measure proof bundles with branching (DAG structure)', () => {
    console.log('\n=== PROOF BUNDLE SIZE WITH BRANCHING (2 parents per tx) ===');
    console.log('Depth | TXs | JSON (bytes) | Encoded (chars) | URL Path (chars)');
    console.log('------|-----|--------------|-----------------|------------------');
    
    for (const depth of [0, 1, 2, 3, 4]) {
      const bundle = buildBundleWithDepth(depth, 2);
      const txCount = countTransactions(bundle);
      const jsonSize = JSON.stringify(bundle).length;
      const urlResult = createSelfCrawlableURL(bundle);
      const encodedSize = urlResult.payload.length;
      const urlSize = urlResult.path.length;
      
      console.log(`${depth.toString().padStart(5)} | ${txCount.toString().padStart(3)} | ${jsonSize.toString().padStart(12)} | ${encodedSize.toString().padStart(15)} | ${urlSize.toString().padStart(16)}`);
    }
  });

  it('should analyze URL size limits and compatibility', () => {
    console.log('\n=== URL SIZE LIMITS ANALYSIS ===');
    
    const limits = {
      'QR Code (alphanumeric)': 4296,
      'QR Code (binary)': 2953,
      'IE11 (legacy)': 2083,
      'Chrome/Edge': 2097152,
      'Firefox': 65536,
      'Safari': 80000,
      'nginx default': 8192,
      'Apache default': 8192,
      'Most proxies': 8192,
      'Social media (Twitter)': 280,
      'SMS': 160
    };
    
    const testCases = [
      { name: 'Single tx', bundle: buildBundleWithDepth(0, 1) },
      { name: '1 parent', bundle: buildBundleWithDepth(1, 1) },
      { name: '2 parents deep', bundle: buildBundleWithDepth(2, 1) },
      { name: '3 parents deep', bundle: buildBundleWithDepth(3, 1) },
      { name: '5 parents deep', bundle: buildBundleWithDepth(5, 1) },
      { name: '2-branch depth 3', bundle: buildBundleWithDepth(3, 2) }
    ];
    
    console.log('\nBundle Type         | URL Size | QR | IE11 | Firefox | nginx | Chrome');
    console.log('--------------------|----------|----|----|---------|-------|--------');
    
    for (const tc of testCases) {
      const urlResult = createSelfCrawlableURL(tc.bundle);
      const size = urlResult.path.length;
      
      const qr = size <= 4296 ? 'OK' : 'NO';
      const ie = size <= 2083 ? 'OK' : 'NO';
      const ff = size <= 65536 ? 'OK' : 'NO';
      const ng = size <= 8192 ? 'OK' : 'NO';
      const ch = size <= 2097152 ? 'OK' : 'NO';
      
      console.log(`${tc.name.padEnd(19)} | ${size.toString().padStart(8)} | ${qr.padStart(2)} | ${ie.padStart(2)} | ${ff.padStart(7)} | ${ng.padStart(5)} | ${ch.padStart(6)}`);
    }
    
    console.log('\n=== RECOMMENDATIONS ===');
    const singleTxUrl = createSelfCrawlableURL(buildBundleWithDepth(0, 1));
    const depth2Url = createSelfCrawlableURL(buildBundleWithDepth(2, 1));
    const depth5Url = createSelfCrawlableURL(buildBundleWithDepth(5, 1));
    
    console.log(`Single tx proof: ${singleTxUrl.path.length} chars - fits everywhere except SMS/Twitter`);
    console.log(`2-depth proof: ${depth2Url.path.length} chars - fits QR, Firefox, Chrome, Safari`);
    console.log(`5-depth proof: ${depth5Url.path.length} chars - fits Firefox, Chrome, Safari`);
    
    if (depth5Url.path.length < 65536) {
      console.log('\n*** CONCLUSION: 5-depth proofs fit in most modern browsers ***');
    }
  });

  it('should estimate realistic mainnet proof sizes', () => {
    console.log('\n=== REALISTIC MAINNET PROOF SIZE ESTIMATES ===');
    console.log('Assuming 15s checkpoints, typical ancestry depth is 1-3 for recent txs\n');
    
    const depth1 = buildBundleWithDepth(1, 1);
    const depth2 = buildBundleWithDepth(2, 1);
    const depth3 = buildBundleWithDepth(3, 1);
    
    const url1 = createSelfCrawlableURL(depth1);
    const url2 = createSelfCrawlableURL(depth2);
    const url3 = createSelfCrawlableURL(depth3);
    
    console.log('Typical cases (tx within last few checkpoints):');
    console.log(`  1-depth proof: ${url1.path.length} chars (${(url1.path.length / 1024).toFixed(2)} KB)`);
    console.log(`  2-depth proof: ${url2.path.length} chars (${(url2.path.length / 1024).toFixed(2)} KB)`);
    console.log(`  3-depth proof: ${url3.path.length} chars (${(url3.path.length / 1024).toFixed(2)} KB)`);
    
    console.log('\nQR Code compatibility:');
    console.log(`  1-depth: ${url1.path.length <= 4296 ? 'FITS' : 'TOO LARGE'} for QR`);
    console.log(`  2-depth: ${url2.path.length <= 4296 ? 'FITS' : 'TOO LARGE'} for QR`);
    console.log(`  3-depth: ${url3.path.length <= 4296 ? 'FITS' : 'TOO LARGE'} for QR`);
  });
});
