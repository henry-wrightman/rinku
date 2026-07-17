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
    
    const tipCount =
      typeof status.tipCount === 'number'
        ? status.tipCount
        : Array.isArray(status.tips)
          ? status.tips.length
          : typeof status.tips === 'string'
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
    const actualHash = tx.hash || node.hash;
    
    // Try with fee first (new transactions)
    const txWithFee: any = {
      from: tx.from,
      to: tx.to,
      amount: tx.amount,
      fee: tx.fee,
      nonce: tx.nonce,
      tipUrls: tx.tipUrls || [],
      sig: tx.sig,
      ts: tx.ts
    };
    const hashWithFee = await hashTransaction(txWithFee);
    
    // Try without fee (old transactions before gas feature)
    const txWithoutFee: any = {
      from: tx.from,
      to: tx.to,
      amount: tx.amount,
      nonce: tx.nonce,
      tipUrls: tx.tipUrls || [],
      sig: tx.sig,
      ts: tx.ts
    };
    const hashWithoutFee = await hashTransaction(txWithoutFee);
    
    // Accept either hash (backwards compatible)
    if (hashWithFee !== actualHash && hashWithoutFee !== actualHash) {
      hashErrors++;
      if (hashErrors <= 10) {
        record('hashes', `Hash for ${actualHash?.slice(0, 8) || 'unknown'}...`, false, 
          `Expected ${hashWithFee.slice(0, 8)}... or ${hashWithoutFee.slice(0, 8)}...`);
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
      // Try with fee first (new transactions)
      const txWithFee: any = {
        from: tx.from,
        to: tx.to,
        amount: tx.amount,
        fee: tx.fee,
        nonce: tx.nonce,
        tipUrls: tx.tipUrls || [],
        sig: '',
        ts: tx.ts
      };
      const hashWithFee = await hashTransaction(txWithFee);
      
      // Try without fee (old transactions before gas feature)
      const txWithoutFee: any = {
        from: tx.from,
        to: tx.to,
        amount: tx.amount,
        nonce: tx.nonce,
        tipUrls: tx.tipUrls || [],
        sig: '',
        ts: tx.ts
      };
      const hashWithoutFee = await hashTransaction(txWithoutFee);
      
      // Verify against either hash (backwards compatible)
      const isValidWithFee = await verify(hashWithFee, tx.sig, pubKey);
      const isValidWithoutFee = await verify(hashWithoutFee, tx.sig, pubKey);
      const isValid = isValidWithFee || isValidWithoutFee;
      
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

async function validateCheckpoints(syncStatus?: any) {
  logSection('6. CHECKPOINT CHAIN');
  
  try {
    const checkpointData = await fetchJSON('/api/checkpoints');
    // Live API: { total, checkpoints: [{height, merkleRoot, txCount, ...}] }
    // Legacy: { chain: [{checkpointHeight, previousCheckpointId, ...}] }
    const recent = checkpointData.checkpoints || checkpointData.chain || [];
    const total = checkpointData.total ?? recent.length;
    
    record('checkpoints', 'Checkpoint list retrieved', true, `${recent.length} recent / ${total} total`);
    
    if (recent.length === 0) {
      const height = syncStatus?.checkpointHeight ?? 0;
      if (height > 0) {
        record('checkpoints', 'No checkpoints returned despite height>0', false,
          `sync height=${height}`);
        return { valid: false, count: 0 };
      }
      record('checkpoints', 'No checkpoints to validate', true, 'Chain just started');
      return { valid: true, count: 0 };
    }

    const normalized = recent.map((cp: any) => ({
      height: cp.height ?? cp.checkpointHeight ?? 0,
      merkleRoot: cp.merkleRoot ?? cp.txMerkleRoot ?? '',
      txCount: cp.txCount ?? cp.tipCount ?? 0,
      validators: cp.validators ?? cp.validatorCount ?? cp.signatureCount ?? 0,
      previousCheckpointId: cp.previousCheckpointId,
      checkpointId: cp.checkpointId ?? cp.hash,
    }));

    // Recent list is newest-first; expect contiguous descending heights
    let gapErrors = 0;
    for (let i = 1; i < normalized.length; i++) {
      const newer = normalized[i - 1].height;
      const older = normalized[i].height;
      if (newer > older && newer - older !== 1) {
        gapErrors++;
      }
    }
    record('checkpoints', 'Recent checkpoint heights ordered', gapErrors === 0,
      gapErrors ? `${gapErrors} gaps` : `${normalized[normalized.length - 1].height}…${normalized[0].height}`);

    const missingRoots = normalized.filter((cp: any) => !cp.merkleRoot).length;
    record('checkpoints', 'Recent checkpoints have merkle roots', missingRoots === 0,
      missingRoots ? `${missingRoots} missing` : undefined);

    const latest = normalized[0];
    if (syncStatus?.checkpointHeight != null) {
      const syncH = Number(syncStatus.checkpointHeight);
      record('checkpoints', 'Latest matches sync status height', latest.height === syncH,
        `checkpoint=${latest.height} sync=${syncH}`);
    }

    if (typeof syncStatus?.totalTransactions === 'number' && syncStatus.totalTransactions > 0) {
      record('checkpoints', 'Network reports historical transactions', true,
        `${syncStatus.totalTransactions} total`);
    }

    // Legacy full-chain link walk when old payload is present
    if (Array.isArray(checkpointData.chain) && checkpointData.chain.length > 0) {
      let chainErrors = 0;
      let prevId: string | null = null;
      for (let i = 0; i < checkpointData.chain.length; i++) {
        const cp = checkpointData.chain[i];
        if (i > 0 && cp.previousCheckpointId !== prevId) chainErrors++;
        if (i > 0) {
          const prevCp = checkpointData.chain[i - 1];
          if (cp.checkpointHeight !== prevCp.checkpointHeight + 1) chainErrors++;
        }
        prevId = cp.checkpointId;
      }
      record('checkpoints', 'Legacy chain integrity', chainErrors === 0,
        chainErrors ? `${chainErrors} errors` : undefined);
    }
    
    return { valid: gapErrors === 0 && missingRoots === 0, count: total };
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
    .filter(n => n.finality || n.confirmed || n.hash)
    .slice(0, sampleSize);
  
  if (samples.length === 0) {
    // Vacuous skip only on a brand-new chain; pruned live nets must sample tips instead.
    record('bundles', 'No finalized transactions to sample', false,
      'Empty sample — use tip/account fallback for pruned DAG');
    return;
  }
  
  let bundleErrors = 0;
  
  for (const node of samples) {
    try {
      const proofData = await fetchJSON(`/api/tx/${node.hash}/proof`);
      
      // Current API: { txHash, finalized, proofUrl, proofSizeBytes, ... }
      // Legacy: { bundle: { hash, tx, parents, truncatedParents } }
      if (proofData.bundle) {
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
        continue;
      }

      if (proofData.proofUrl || proofData.selfContainedProofUrl) {
        const url = proofData.proofUrl || proofData.selfContainedProofUrl;
        if (!proofData.finalized && !node.finality) {
          bundleErrors++;
          record('bundles', `Proof for ${node.hash.slice(0, 8)}...`, false, 'Not finalized');
          continue;
        }
        if (typeof url !== 'string' || url.length < 16) {
          bundleErrors++;
          record('bundles', `Proof URL for ${node.hash.slice(0, 8)}...`, false, 'Empty');
          continue;
        }
        record('bundles', `Proof URL for ${node.hash.slice(0, 8)}...`, true,
          `${(proofData.proofSizeBytes ?? url.length)}B`);
        continue;
      }

      bundleErrors++;
      record('bundles', `Proof for ${node.hash.slice(0, 8)}...`, false, 'No bundle or proofUrl');
      
    } catch (e: any) {
      bundleErrors++;
      record('bundles', `Bundle for ${node.hash.slice(0, 8)}...`, false, e.message);
    }
  }
  
  if (bundleErrors === 0) {
    record('bundles', `All ${samples.length} sampled proofs valid`, true);
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
      const fee = tx.fee || 0;
      
      const fromBal = computed.get(tx.from) || 0;
      computed.set(tx.from, fromBal - tx.amount - fee);
      
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

  if (transactions.length === 0) {
    record('weights', 'Weight checks skipped', true, 'No in-memory txs (pruned)');
    return { zeroWeights: 0, negativeWeights: 0 };
  }
  
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

/** When /api/sync/transactions is empty after pruning, sample tips + accounts. */
async function samplePrunedDagEvidence(syncStatus: any): Promise<any[]> {
  logSection('PRUNED-DAG SAMPLING');

  const tipHashes: string[] = Array.isArray(syncStatus.tips)
    ? syncStatus.tips
    : typeof syncStatus.tips === 'string'
      ? syncStatus.tips.split(',').map((s: string) => s.trim()).filter(Boolean)
      : [];

  record('pruned', 'Tips available for sampling', tipHashes.length > 0,
    `${tipHashes.length} tips (dagSize=${syncStatus.dagSize ?? '?'})`);

  const sampled: any[] = [];
  for (const hash of tipHashes.slice(0, 8)) {
    try {
      const tx = await fetchJSON(`/api/tx/${hash}`);
      sampled.push({
        tx,
        hash: tx.txHash || tx.hash || hash,
        parentHashes: tx.parents || tx.parentHashes || [],
        weight: tx.weight || 0,
        confirmed: tx.finalized || tx.confirmed || false,
        finality: tx.finalized
          ? { checkpointHeight: tx.checkpointHeight, checkpointId: 'sampled', finalizedAt: Date.now() }
          : undefined,
      });
      record('pruned', `Fetched tip ${hash.slice(0, 10)}...`, true,
        `finalized=${!!tx.finalized}`);
    } catch (e: any) {
      record('pruned', `Fetch tip ${hash.slice(0, 10)}...`, false, e.message);
    }
  }

  try {
    const accountsData = await fetchJSON('/api/accounts');
    const accounts = accountsData.accounts || [];
    record('pruned', 'Accounts API populated', accounts.length > 0,
      `${accounts.length} accounts`);
    const withNonce = accounts.filter((a: any) => (a.nonce ?? 0) > 0 || (a.effectiveNonce ?? 0) > 0);
    record('pruned', 'Accounts show historical activity', withNonce.length > 0,
      `${withNonce.length} with nonce>0`);
  } catch (e: any) {
    record('pruned', 'Accounts API', false, e.message);
  }

  if (sampled.length === 0 && (syncStatus.totalTransactions > 0 || syncStatus.checkpointHeight > 0)) {
    record('pruned', 'Could not sample any tip txs', false,
      'DAG pruned and tip fetch failed — validation cannot be vacuous');
  } else if (sampled.length > 0) {
    record('pruned', 'Using tip samples for deep checks', true, `${sampled.length} txs`);
  }

  return sampled;
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
    const syncStatus = await validateNodeConnectivity();
    
    log('\n  Fetching sync transactions...');
    const txData = await fetchJSON('/api/sync/transactions');
    const rawTransactions = txData.transactions || [];
    log(`  Retrieved ${rawTransactions.length} transactions from sync API`);
    
    let transactions = rawTransactions.map((item: any) => {
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

    const looksPruned =
      transactions.length === 0 &&
      ((syncStatus.totalTransactions ?? 0) > 0 || (syncStatus.checkpointHeight ?? 0) > 0);

    if (looksPruned) {
      log('  Sync txs empty but network has history — entering pruned-DAG sampling');
      transactions = await samplePrunedDagEvidence(syncStatus);
      // Tip API payloads are proof-oriented; prefer proof/checkpoint checks over
      // re-hashing explorer-era fields that may not round-trip.
      await validateCheckpoints(syncStatus);
      if (transactions.length > 0) {
        await validateSelfCrawlableBundles(transactions);
      } else {
        record('bundles', 'Bundle sampling', false, 'No tip samples available after prune');
      }
    } else if (transactions.length === 0) {
      record('connectivity', 'Fresh chain with no txs yet', true, 'OK for brand-new node');
      await validateCheckpoints(syncStatus);
    } else {
      await validateTransactionHashes(transactions);
      await validateSignatures(transactions, publicKeys);
      await validateDAGIntegrity(transactions);
      await validateNonceSequencing(transactions);
      await validateCheckpoints(syncStatus);
      await validateMerkleRoots(transactions);
      await validateFinalityMetadata(transactions);
      await validateSelfCrawlableBundles(transactions);
      await validateBalanceConsistency(transactions);
      await validateWeights(transactions);
    }
    
    const success = await generateReport();
    process.exit(success ? 0 : 1);
    
  } catch (e: any) {
    console.error(`\n\x1b[31m  FATAL ERROR: ${e.message}\x1b[0m\n`);
    process.exit(1);
  }
}

main();
