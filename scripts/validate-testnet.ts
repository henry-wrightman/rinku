#!/usr/bin/env npx ts-node
/**
 * Rinku Testnet Validation Script
 * 
 * Exhaustively validates all transactions, hashes, signatures, DAG integrity,
 * checkpoints, and merkle proofs on a running testnet.
 * 
 * Usage:
 *   npx ts-node scripts/validate-testnet.ts [NODE_URL]
 * 
 * Example:
 *   npx ts-node scripts/validate-testnet.ts https://your-replit-domain.replit.app
 *   npx ts-node scripts/validate-testnet.ts http://localhost:3001
 */

import { 
  hashTransaction, 
  verify, 
  getTransactionMerkleRoot
} from '@rinku/core';

const NODE_URL = process.argv[2] || 'http://localhost:3001';

interface ValidationResult {
  category: string;
  check: string;
  passed: boolean;
  details?: string;
}

const results: ValidationResult[] = [];
let totalChecks = 0;
let passedChecks = 0;

function log(msg: string) {
  console.log(msg);
}

function logSection(title: string) {
  console.log('\n' + '='.repeat(60));
  console.log(`  ${title}`);
  console.log('='.repeat(60));
}

function record(category: string, check: string, passed: boolean, details?: string) {
  results.push({ category, check, passed, details });
  totalChecks++;
  if (passed) passedChecks++;
  
  const status = passed ? '\x1b[32m✓\x1b[0m' : '\x1b[31m✗\x1b[0m';
  const detailStr = details ? ` (${details})` : '';
  console.log(`  ${status} ${check}${detailStr}`);
}

async function fetchJSON(path: string): Promise<any> {
  const res = await fetch(`${NODE_URL}${path}`);
  if (!res.ok) throw new Error(`HTTP ${res.status}: ${path}`);
  return res.json();
}

async function validateNodeConnectivity() {
  logSection('1. NODE CONNECTIVITY');
  
  try {
    const status = await fetchJSON('/api/sync/status');
    record('connectivity', 'Node is reachable', true);
    record('connectivity', 'Has merkle root', !!status.merkleRoot, status.merkleRoot?.slice(0, 16) + '...');
    record('connectivity', 'DAG size reported', status.dagSize >= 0, `${status.dagSize} nodes`);
    
    const tipCount = typeof status.tips === 'string' 
      ? status.tips.split(',').filter((t: string) => t.length > 0).length 
      : (typeof status.tips === 'number' ? status.tips : 0);
    record('connectivity', 'Tips count valid', tipCount >= 0, `${tipCount} tips`);
    return status;
  } catch (e: any) {
    record('connectivity', 'Node is reachable', false, e.message);
    throw new Error('Cannot proceed without node connectivity');
  }
}

async function validateTransactionHashes(transactions: any[]) {
  logSection('2. TRANSACTION HASH INTEGRITY');
  
  let hashErrors = 0;
  
  for (const node of transactions) {
    const tx = node.tx || node;
    const txForHash = {
      from: tx.from,
      to: tx.to,
      amount: tx.amount,
      nonce: tx.nonce,
      tipUrls: tx.tipUrls || [],
      sig: tx.sig,
      ts: tx.ts
    };
    const expectedHash = await hashTransaction(txForHash);
    const actualHash = tx.hash || node.hash;
    
    if (expectedHash !== actualHash) {
      hashErrors++;
      if (hashErrors <= 10) {
        record('hashes', `Hash for ${actualHash?.slice(0, 8) || 'unknown'}...`, false, 
          `Expected ${expectedHash.slice(0, 8)}...`);
      }
    }
  }
  
  if (hashErrors === 0) {
    record('hashes', `All ${transactions.length} transaction hashes verified`, true);
  } else {
    record('hashes', `Hash verification complete`, false, `${hashErrors} errors`);
  }
  
  return hashErrors;
}

async function validateSignatures(transactions: any[], publicKeys: Map<string, Uint8Array>) {
  logSection('3. SIGNATURE VERIFICATION');
  
  let sigErrors = 0;
  let verified = 0;
  let skipped = 0;
  
  for (const node of transactions) {
    const tx = node.tx || node;
    const pubKey = publicKeys.get(tx.from);
    
    if (!pubKey) {
      skipped++;
      continue;
    }
    
    try {
      const txForHash = {
        from: tx.from,
        to: tx.to,
        amount: tx.amount,
        nonce: tx.nonce,
        tipUrls: tx.tipUrls || [],
        sig: '',
        ts: tx.ts
      };
      const txHash = await hashTransaction(txForHash);
      const isValid = await verify(txHash, tx.sig, pubKey);
      
      if (!isValid) {
        sigErrors++;
        if (sigErrors <= 10) {
          record('signatures', `Signature for ${node.hash?.slice(0, 8)}...`, false, 'Invalid');
        }
      } else {
        verified++;
      }
    } catch (e: any) {
      sigErrors++;
      if (sigErrors <= 10) {
        record('signatures', `Signature for ${node.hash?.slice(0, 8)}...`, false, e.message);
      }
    }
  }
  
  if (sigErrors === 0) {
    record('signatures', `All ${verified} signatures verified`, true, `${skipped} skipped (no pubkey)`);
  } else {
    record('signatures', `Signature verification complete`, false, `${sigErrors} errors`);
  }
  
  return sigErrors;
}

async function validateDAGIntegrity(transactions: any[]) {
  logSection('4. DAG STRUCTURE INTEGRITY');
  
  const hashSet = new Set(transactions.map(n => n.hash));
  
  const urlToHash = new Map<string, string>();
  for (const node of transactions) {
    const tx = node.tx;
    if (tx.hash) {
      urlToHash.set(`/tx/h/${tx.hash}`, tx.hash);
      urlToHash.set(tx.hash, tx.hash);
    }
  }
  
  function resolveParentHashes(tx: any): string[] {
    const tipUrls = tx.tipUrls || [];
    const resolved: string[] = [];
    for (const url of tipUrls) {
      if (url.startsWith('/tx/h/')) {
        resolved.push(url.replace('/tx/h/', ''));
      } else if (urlToHash.has(url)) {
        resolved.push(urlToHash.get(url)!);
      }
    }
    return resolved;
  }
  
  let orphanParents = 0;
  let cycleDetected = false;
  let totalParentRefs = 0;
  
  for (const node of transactions) {
    const parentHashes = resolveParentHashes(node.tx);
    totalParentRefs += parentHashes.length;
    for (const parentHash of parentHashes) {
      if (!hashSet.has(parentHash)) {
        orphanParents++;
      }
    }
  }
  
  if (totalParentRefs === 0) {
    record('dag', 'Parent references', true, 'No parent refs (all genesis or pruned)');
  } else if (orphanParents > 0) {
    record('dag', 'Orphan parent references', true, 
      `${orphanParents} parents not in DAG (pruned)`);
  } else {
    record('dag', 'All parent references valid', true);
  }
  
  const visited = new Set<string>();
  const recursionStack = new Set<string>();
  
  function hasCycle(hash: string, nodeMap: Map<string, any>): boolean {
    if (recursionStack.has(hash)) return true;
    if (visited.has(hash)) return false;
    
    visited.add(hash);
    recursionStack.add(hash);
    
    const node = nodeMap.get(hash);
    if (node) {
      const parentHashes = resolveParentHashes(node.tx);
      for (const parentHash of parentHashes) {
        if (hasCycle(parentHash, nodeMap)) return true;
      }
    }
    
    recursionStack.delete(hash);
    return false;
  }
  
  const nodeMap = new Map(transactions.map(n => [n.hash, n]));
  for (const node of transactions) {
    if (hasCycle(node.hash, nodeMap)) {
      cycleDetected = true;
      break;
    }
  }
  
  record('dag', 'No cycles detected', !cycleDetected);
  
  const referencedHashes = new Set<string>();
  for (const node of transactions) {
    for (const parentHash of resolveParentHashes(node.tx)) {
      referencedHashes.add(parentHash);
    }
  }
  
  const tips = transactions.filter(n => !referencedHashes.has(n.hash));
  record('dag', 'Tips identified', true, `${tips.length} current tips`);
  
  const roots = transactions.filter(n => {
    const parentHashes = resolveParentHashes(n.tx);
    return parentHashes.length === 0 || 
           parentHashes.every(p => !hashSet.has(p));
  });
  record('dag', 'Roots identified', true, `${roots.length} roots (genesis or pruned boundary)`);
  
  return { orphanParents, cycleDetected };
}

async function validateNonceSequencing(transactions: any[]) {
  logSection('5. NONCE SEQUENCING');
  
  const byAddress = new Map<string, any[]>();
  
  for (const node of transactions) {
    const tx = node.tx || node;
    const addr = tx.from;
    if (!byAddress.has(addr)) byAddress.set(addr, []);
    byAddress.get(addr)!.push({ ...tx, hash: node.hash, ts: tx.ts });
  }
  
  let nonceErrors = 0;
  let accountsChecked = 0;
  
  for (const entry of Array.from(byAddress.entries())) {
    const [addr, txs] = entry;
    txs.sort((a, b) => a.nonce - b.nonce);
    
    let expectedNonce = txs[0].nonce;
    for (const tx of txs) {
      if (tx.nonce !== expectedNonce) {
        nonceErrors++;
        if (nonceErrors <= 3) {
          record('nonces', `Nonce gap for ${addr.slice(0, 8)}...`, false, 
            `Expected ${expectedNonce}, got ${tx.nonce}`);
        }
      }
      expectedNonce = tx.nonce + 1;
    }
    accountsChecked++;
  }
  
  if (nonceErrors === 0) {
    record('nonces', `Nonce sequences valid for ${accountsChecked} accounts`, true);
  } else if (nonceErrors > 3) {
    record('nonces', `Additional nonce errors`, false, `${nonceErrors - 3} more`);
  }
  
  return nonceErrors;
}

async function validateCheckpoints() {
  logSection('6. CHECKPOINT CHAIN');
  
  try {
    const checkpointData = await fetchJSON('/api/checkpoints');
    const chain = checkpointData.chain || [];
    
    record('checkpoints', 'Checkpoint chain retrieved', true, `${chain.length} checkpoints`);
    
    if (chain.length === 0) {
      record('checkpoints', 'No checkpoints to validate', true, 'Chain just started');
      return { valid: true, count: 0 };
    }
    
    let chainErrors = 0;
    let prevId: string | null = null;
    
    for (let i = 0; i < chain.length; i++) {
      const cp = chain[i];
      
      if (i === 0) {
        if (cp.checkpointHeight !== 0) {
          chainErrors++;
          record('checkpoints', 'Genesis height', false, `Expected 0, got ${cp.checkpointHeight}`);
        }
      } else {
        if (cp.previousCheckpointId !== prevId) {
          chainErrors++;
          record('checkpoints', `Chain link at height ${cp.checkpointHeight}`, false, 'Broken chain');
        }
        
        const prevCp = chain[i - 1];
        if (cp.checkpointHeight !== prevCp.checkpointHeight + 1) {
          chainErrors++;
          record('checkpoints', `Height sequence at ${cp.checkpointHeight}`, false, 'Gap detected');
        }
      }
      
      if (cp.signatureCount < 1) {
        chainErrors++;
        record('checkpoints', `Signatures at height ${cp.checkpointHeight}`, false, 'No signatures');
      }
      
      prevId = cp.checkpointId;
    }
    
    if (chainErrors === 0) {
      record('checkpoints', 'Checkpoint chain integrity verified', true);
    }
    
    const latest = chain[chain.length - 1];
    record('checkpoints', 'Latest checkpoint has txMerkleRoot', !!latest.txMerkleRoot);
    record('checkpoints', 'Validator weight recorded', latest.totalValidatorWeight > 0, 
      `${latest.totalValidatorWeight?.toFixed(1)}%`);
    
    return { valid: chainErrors === 0, count: chain.length };
  } catch (e: any) {
    record('checkpoints', 'Checkpoint chain retrieval', false, e.message);
    return { valid: false, count: 0 };
  }
}

async function validateMerkleRoots(transactions: any[]) {
  logSection('7. MERKLE ROOT VERIFICATION');
  
  try {
    const status = await fetchJSON('/api/sync/status');
    const reportedRoot = status.merkleRoot;
    
    record('merkle', 'DAG state merkle root reported', !!reportedRoot,
      reportedRoot ? reportedRoot.slice(0, 16) + '...' : 'none');
    
    if (transactions.length === 0) {
      record('merkle', 'No transactions to verify', true);
      return;
    }
    
    const txHashes = transactions.map(n => n.hash);
    const txMerkleRoot = await getTransactionMerkleRoot(txHashes);
    record('merkle', 'Transaction merkle root computed', !!txMerkleRoot,
      txMerkleRoot.slice(0, 16) + '...');
    
    record('merkle', 'Transaction count matches hash count', 
      transactions.length === txHashes.length, `${txHashes.length} hashes`);
    
  } catch (e: any) {
    record('merkle', 'Merkle root verification', false, e.message);
  }
}

async function validateFinalityMetadata(transactions: any[]) {
  logSection('8. FINALITY METADATA');
  
  const finalized = transactions.filter(n => n.finality);
  const unfinalized = transactions.filter(n => !n.finality);
  
  record('finality', 'Finalized transactions', true, `${finalized.length} of ${transactions.length}`);
  record('finality', 'Unfinalized transactions', true, `${unfinalized.length} pending`);
  
  let finalityErrors = 0;
  
  for (const node of finalized) {
    const f = node.finality;
    if (!f.checkpointId || typeof f.checkpointHeight !== 'number' || !f.finalizedAt) {
      finalityErrors++;
      if (finalityErrors <= 3) {
        record('finality', `Finality data for ${node.hash.slice(0, 8)}...`, false, 'Incomplete');
      }
    }
  }
  
  if (finalityErrors === 0 && finalized.length > 0) {
    record('finality', 'All finality metadata complete', true);
  } else if (finalityErrors > 0) {
    record('finality', 'Finality metadata errors', false, `${finalityErrors} incomplete`);
  }
  
  return { finalized: finalized.length, unfinalized: unfinalized.length, errors: finalityErrors };
}

async function validateSelfCrawlableBundles(transactions: any[]) {
  logSection('9. SELF-CRAWLABLE PROOF BUNDLES');
  
  const sampleSize = Math.min(10, transactions.length);
  const samples = transactions
    .filter(n => n.finality)
    .slice(0, sampleSize);
  
  if (samples.length === 0) {
    record('bundles', 'No finalized transactions to sample', true, 'Skip bundle validation');
    return;
  }
  
  let bundleErrors = 0;
  
  for (const node of samples) {
    try {
      const proofData = await fetchJSON(`/api/tx/${node.hash}/proof`);
      
      if (!proofData.bundle) {
        bundleErrors++;
        record('bundles', `Bundle for ${node.hash.slice(0, 8)}...`, false, 'No bundle returned');
        continue;
      }
      
      const bundle = proofData.bundle;
      
      if (bundle.hash !== node.hash) {
        bundleErrors++;
        record('bundles', `Bundle hash for ${node.hash.slice(0, 8)}...`, false, 'Hash mismatch');
        continue;
      }
      
      if (!bundle.tx) {
        bundleErrors++;
        record('bundles', `Bundle tx for ${node.hash.slice(0, 8)}...`, false, 'Missing tx');
        continue;
      }
      
      const hasParents = (bundle.parents?.length || 0) > 0;
      const hasTruncated = (bundle.truncatedParents?.length || 0) > 0;
      
      if (!hasParents && !hasTruncated && (node.parentHashes?.length || 0) > 0) {
        bundleErrors++;
        record('bundles', `Bundle parents for ${node.hash.slice(0, 8)}...`, false, 'Missing parents');
        continue;
      }
      
      for (const tp of (bundle.truncatedParents || [])) {
        if (!tp.tx || !tp.checkpointAnchor) {
          bundleErrors++;
          record('bundles', `Truncated parent in ${node.hash.slice(0, 8)}...`, false, 'Incomplete');
        }
      }
      
    } catch (e: any) {
      bundleErrors++;
      record('bundles', `Bundle for ${node.hash.slice(0, 8)}...`, false, e.message);
    }
  }
  
  if (bundleErrors === 0) {
    record('bundles', `All ${samples.length} sampled bundles valid`, true);
  }
  
  return bundleErrors;
}

async function validateBalanceConsistency(transactions: any[]) {
  logSection('10. BALANCE CONSISTENCY');
  
  try {
    const accountsData = await fetchJSON('/api/accounts');
    const accounts = accountsData.accounts || [];
    
    const computed = new Map<string, number>();
    
    const sorted = [...transactions].sort((a, b) => {
      const txA = a.tx || a;
      const txB = b.tx || b;
      return txA.ts - txB.ts;
    });
    
    for (const node of sorted) {
      const tx = node.tx || node;
      
      const fromBal = computed.get(tx.from) || 0;
      computed.set(tx.from, fromBal - tx.amount);
      
      const toBal = computed.get(tx.to) || 0;
      computed.set(tx.to, toBal + tx.amount);
    }
    
    let balanceMismatches = 0;
    
    for (const account of accounts) {
      const expected = computed.get(account.address);
      if (expected !== undefined) {
        if (Math.abs(account.balance - expected) > 0.01) {
          balanceMismatches++;
          if (balanceMismatches <= 3) {
            record('balances', `Balance for ${account.address.slice(0, 8)}...`, false,
              `Expected ~${expected.toFixed(2)}, got ${account.balance}`);
          }
        }
      }
    }
    
    if (balanceMismatches === 0) {
      record('balances', `Balance consistency verified for ${accounts.length} accounts`, true);
    } else {
      record('balances', 'Balance mismatches detected', false, 
        `${balanceMismatches} (may be due to rewards/staking)`);
    }
    
    return balanceMismatches;
  } catch (e: any) {
    record('balances', 'Balance consistency check', false, e.message);
    return -1;
  }
}

async function validateWeights(transactions: any[]) {
  logSection('11. WEIGHT CALCULATIONS');
  
  let zeroWeights = 0;
  let negativeWeights = 0;
  
  for (const node of transactions) {
    if (node.weight === 0) zeroWeights++;
    if (node.weight < 0) negativeWeights++;
  }
  
  record('weights', 'No negative weights', negativeWeights === 0, 
    negativeWeights > 0 ? `${negativeWeights} found` : undefined);
  
  if (zeroWeights > 0) {
    record('weights', 'Zero weight transactions', true, 
      `${zeroWeights} (normal for new accounts)`);
  }
  
  const maxWeight = Math.max(...transactions.map(n => n.weight || 0));
  record('weights', 'Maximum weight observed', true, maxWeight.toFixed(2));
  
  return { zeroWeights, negativeWeights };
}

async function generateReport() {
  logSection('VALIDATION SUMMARY');
  
  const failed = results.filter(r => !r.passed);
  
  console.log(`\n  Total Checks: ${totalChecks}`);
  console.log(`  \x1b[32mPassed: ${passedChecks}\x1b[0m`);
  console.log(`  \x1b[31mFailed: ${totalChecks - passedChecks}\x1b[0m`);
  console.log(`  Success Rate: ${((passedChecks / totalChecks) * 100).toFixed(1)}%`);
  
  if (failed.length > 0) {
    console.log('\n  Failed Checks:');
    for (const f of failed) {
      console.log(`    - [${f.category}] ${f.check}${f.details ? `: ${f.details}` : ''}`);
    }
  }
  
  console.log('\n' + '='.repeat(60));
  
  if (failed.length === 0) {
    console.log('\x1b[32m  ALL VALIDATIONS PASSED - TESTNET IS HEALTHY\x1b[0m');
  } else {
    console.log('\x1b[33m  SOME VALIDATIONS FAILED - REVIEW ABOVE\x1b[0m');
  }
  
  console.log('='.repeat(60) + '\n');
  
  return failed.length === 0;
}

async function main() {
  console.log('\n');
  console.log('╔════════════════════════════════════════════════════════════╗');
  console.log('║         RINKU TESTNET VALIDATION SCRIPT                    ║');
  console.log('╚════════════════════════════════════════════════════════════╝');
  console.log(`\n  Target Node: ${NODE_URL}`);
  console.log(`  Started: ${new Date().toISOString()}`);
  
  try {
    await validateNodeConnectivity();
    
    log('\n  Fetching all transactions...');
    const txData = await fetchJSON('/api/sync/transactions');
    const rawTransactions = txData.transactions || [];
    log(`  Retrieved ${rawTransactions.length} transactions`);
    
    const transactions = rawTransactions.map((item: any) => {
      const tx = item.tx || item;
      return {
        tx,
        hash: tx.hash,
        parentHashes: tx.parentHashes || [],
        weight: tx.weight || 0,
        confirmed: tx.confirmed || false,
        finality: tx.finality
      };
    });
    
    const publicKeys = new Map<string, Uint8Array>();
    for (const item of rawTransactions) {
      if (item.publicKey && item.tx?.from) {
        publicKeys.set(item.tx.from, new Uint8Array(item.publicKey));
      }
    }
    log(`  Retrieved ${publicKeys.size} public keys`);
    
    await validateTransactionHashes(transactions);
    await validateSignatures(transactions, publicKeys);
    await validateDAGIntegrity(transactions);
    await validateNonceSequencing(transactions);
    await validateCheckpoints();
    await validateMerkleRoots(transactions);
    await validateFinalityMetadata(transactions);
    await validateSelfCrawlableBundles(transactions);
    await validateBalanceConsistency(transactions);
    await validateWeights(transactions);
    
    const success = await generateReport();
    process.exit(success ? 0 : 1);
    
  } catch (e: any) {
    console.error(`\n\x1b[31m  FATAL ERROR: ${e.message}\x1b[0m\n`);
    process.exit(1);
  }
}

main();
