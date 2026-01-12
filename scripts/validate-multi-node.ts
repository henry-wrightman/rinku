#!/usr/bin/env npx ts-node
/**
 * Rinku Multi-Node Validation Script
 * 
 * Validates consistency across multiple Rinku nodes by comparing:
 * - Merkle roots
 * - Checkpoint chains
 * - Transaction counts
 * - Account balances
 * - Finality status
 * 
 * Usage:
 *   npx ts-node scripts/validate-multi-node.ts NODE1_URL NODE2_URL [NODE3_URL ...]
 * 
 * Examples:
 *   npx ts-node scripts/validate-multi-node.ts https://rinkuchan.com http://localhost:3001
 *   npx ts-node scripts/validate-multi-node.ts https://rinkuchan.com http://localhost:3001 https://rinku-fly.fly.dev
 */

interface NodeStatus {
  url: string;
  name: string;
  reachable: boolean;
  merkleRoot?: string;
  dagSize?: number;
  tips?: number;
  checkpointHeight?: number;
  checkpointId?: string;
  txMerkleRoot?: string;
  latency?: number;
  error?: string;
}

interface ValidationResult {
  check: string;
  passed: boolean;
  details: string;
}

const nodes: string[] = process.argv.slice(2);

if (nodes.length < 2) {
  console.log('Usage: npx ts-node scripts/validate-multi-node.ts NODE1_URL NODE2_URL [NODE3_URL ...]');
  console.log('');
  console.log('Examples:');
  console.log('  npx ts-node scripts/validate-multi-node.ts https://rinkuchan.com http://localhost:3001');
  console.log('  npx ts-node scripts/validate-multi-node.ts https://rinkuchan.com http://localhost:3001 https://rinku-fly.fly.dev');
  process.exit(1);
}

const results: ValidationResult[] = [];
let totalChecks = 0;
let passedChecks = 0;

function logSection(title: string) {
  console.log('\n' + '='.repeat(70));
  console.log(`  ${title}`);
  console.log('='.repeat(70));
}

function record(check: string, passed: boolean, details: string) {
  results.push({ check, passed, details });
  totalChecks++;
  if (passed) passedChecks++;
  
  const status = passed ? '\x1b[32m✓\x1b[0m' : '\x1b[31m✗\x1b[0m';
  console.log(`  ${status} ${check}: ${details}`);
}

async function fetchWithTimeout(url: string, timeoutMs: number = 10000): Promise<any> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  
  try {
    const start = performance.now();
    const res = await fetch(url, { signal: controller.signal });
    const latency = performance.now() - start;
    
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    return { data, latency };
  } finally {
    clearTimeout(timeout);
  }
}

async function getNodeStatus(url: string, index: number): Promise<NodeStatus> {
  const name = `Node ${index + 1}`;
  
  try {
    const { data: status, latency } = await fetchWithTimeout(`${url}/api/sync/status`);
    
    let checkpointHeight: number | undefined;
    let checkpointId: string | undefined;
    let txMerkleRoot: string | undefined;
    
    try {
      const { data: cpData } = await fetchWithTimeout(`${url}/api/checkpoints`);
      const chain = cpData.chain || [];
      if (chain.length > 0) {
        const latest = chain[chain.length - 1];
        checkpointHeight = latest.checkpointHeight;
        checkpointId = latest.checkpointId;
        txMerkleRoot = latest.txMerkleRoot;
      }
    } catch {}
    
    const tipCount = typeof status.tips === 'string'
      ? status.tips.split(',').filter((t: string) => t.length > 0).length
      : (typeof status.tips === 'number' ? status.tips : 0);
    
    return {
      url,
      name,
      reachable: true,
      merkleRoot: status.merkleRoot,
      dagSize: status.dagSize,
      tips: tipCount,
      checkpointHeight,
      checkpointId,
      txMerkleRoot,
      latency,
    };
  } catch (e: any) {
    return {
      url,
      name,
      reachable: false,
      error: e.message,
    };
  }
}

async function validateConnectivity(nodeStatuses: NodeStatus[]) {
  logSection('1. NODE CONNECTIVITY');
  
  for (const node of nodeStatuses) {
    if (node.reachable) {
      record(`${node.name} reachable`, true, `${node.url} (${node.latency?.toFixed(0)}ms)`);
    } else {
      record(`${node.name} reachable`, false, `${node.url} - ${node.error}`);
    }
  }
  
  const reachable = nodeStatuses.filter(n => n.reachable).length;
  record('Minimum nodes online', reachable >= 2, `${reachable}/${nodeStatuses.length} nodes`);
  
  return reachable >= 2;
}

async function validateCheckpointConsensus(nodeStatuses: NodeStatus[]) {
  logSection('2. CHECKPOINT CONSENSUS');
  
  const reachable = nodeStatuses.filter(n => n.reachable);
  
  const heights = reachable.map(n => n.checkpointHeight).filter(h => h !== undefined);
  const uniqueHeights = [...new Set(heights)];
  
  if (heights.length === 0) {
    record('Checkpoint height consensus', true, 'No checkpoints on any node');
    return;
  }
  
  const maxHeight = Math.max(...heights);
  const minHeight = Math.min(...heights);
  const heightDiff = maxHeight - minHeight;
  
  record('Checkpoint height sync', heightDiff <= 1, 
    heightDiff === 0 
      ? `All nodes at height ${maxHeight}`
      : `Height range: ${minHeight}-${maxHeight} (diff: ${heightDiff})`
  );
  
  const nodesAtMax = reachable.filter(n => n.checkpointHeight === maxHeight);
  if (nodesAtMax.length > 1) {
    const checkpointIds = nodesAtMax.map(n => n.checkpointId).filter(Boolean);
    const uniqueIds = [...new Set(checkpointIds)];
    
    record('Checkpoint ID consensus', uniqueIds.length === 1,
      uniqueIds.length === 1
        ? `All nodes agree: ${uniqueIds[0]?.slice(0, 16)}...`
        : `FORK DETECTED: ${uniqueIds.length} different checkpoint IDs`
    );
    
    const txMerkleRoots = nodesAtMax.map(n => n.txMerkleRoot).filter(Boolean);
    const uniqueRoots = [...new Set(txMerkleRoots)];
    
    record('Transaction Merkle root consensus', uniqueRoots.length === 1,
      uniqueRoots.length === 1
        ? `All nodes agree: ${uniqueRoots[0]?.slice(0, 16)}...`
        : `DIVERGENCE: ${uniqueRoots.length} different tx merkle roots`
    );
  }
}

async function validateDAGState(nodeStatuses: NodeStatus[]) {
  logSection('3. DAG STATE COMPARISON');
  
  const reachable = nodeStatuses.filter(n => n.reachable);
  
  const dagSizes = reachable.map(n => ({ name: n.name, size: n.dagSize || 0 }));
  const maxSize = Math.max(...dagSizes.map(d => d.size));
  const minSize = Math.min(...dagSizes.map(d => d.size));
  
  console.log('\n  DAG Sizes:');
  for (const { name, size } of dagSizes) {
    console.log(`    ${name}: ${size} transactions`);
  }
  
  const sizeDiffPercent = maxSize > 0 ? ((maxSize - minSize) / maxSize) * 100 : 0;
  record('DAG size variance', sizeDiffPercent < 10,
    sizeDiffPercent < 1
      ? 'All nodes within 1% variance'
      : `Variance: ${sizeDiffPercent.toFixed(1)}% (${minSize} - ${maxSize} txs)`
  );
  
  const merkleRoots = reachable.map(n => n.merkleRoot).filter(Boolean);
  const uniqueRoots = [...new Set(merkleRoots)];
  
  if (merkleRoots.length > 1) {
    record('DAG merkle root consensus', uniqueRoots.length === 1,
      uniqueRoots.length === 1
        ? `All nodes agree: ${uniqueRoots[0]?.slice(0, 16)}...`
        : `DIVERGENCE: ${uniqueRoots.length} different roots (may be due to sync lag)`
    );
  }
  
  console.log('\n  Tip Counts:');
  for (const node of reachable) {
    console.log(`    ${node.name}: ${node.tips || 0} tips`);
  }
}

async function validateTransactionSync(nodeStatuses: NodeStatus[]) {
  logSection('4. TRANSACTION SYNCHRONIZATION');
  
  const reachable = nodeStatuses.filter(n => n.reachable);
  
  const txCounts = new Map<string, number>();
  const txSets = new Map<string, Set<string>>();
  
  for (const node of reachable) {
    try {
      const { data } = await fetchWithTimeout(`${node.url}/api/sync/transactions`);
      const transactions = data.transactions || [];
      txCounts.set(node.name, transactions.length);
      
      const hashes = new Set<string>(transactions.map((t: any) => t.tx?.hash || t.hash));
      txSets.set(node.name, hashes);
      
      console.log(`  ${node.name}: ${transactions.length} transactions`);
    } catch (e: any) {
      console.log(`  ${node.name}: Failed to fetch - ${e.message}`);
    }
  }
  
  if (txSets.size >= 2) {
    const nodeNames = [...txSets.keys()];
    let allMatch = true;
    
    for (let i = 0; i < nodeNames.length - 1; i++) {
      const set1 = txSets.get(nodeNames[i])!;
      const set2 = txSets.get(nodeNames[i + 1])!;
      
      const only1 = [...set1].filter(h => !set2.has(h));
      const only2 = [...set2].filter(h => !set1.has(h));
      
      if (only1.length > 0 || only2.length > 0) {
        allMatch = false;
        console.log(`\n  Differences between ${nodeNames[i]} and ${nodeNames[i + 1]}:`);
        if (only1.length > 0) {
          console.log(`    Only in ${nodeNames[i]}: ${only1.length} txs`);
          if (only1.length <= 5) {
            for (const h of only1) console.log(`      - ${h.slice(0, 16)}...`);
          }
        }
        if (only2.length > 0) {
          console.log(`    Only in ${nodeNames[i + 1]}: ${only2.length} txs`);
          if (only2.length <= 5) {
            for (const h of only2) console.log(`      - ${h.slice(0, 16)}...`);
          }
        }
      }
    }
    
    record('Transaction set consistency', allMatch,
      allMatch ? 'All nodes have identical transaction sets' : 'Some transactions not synced'
    );
  }
}

async function validateAccountBalances(nodeStatuses: NodeStatus[]) {
  logSection('5. ACCOUNT BALANCE CONSISTENCY');
  
  const reachable = nodeStatuses.filter(n => n.reachable);
  
  const balanceMaps = new Map<string, Map<string, number>>();
  
  for (const node of reachable) {
    try {
      const { data } = await fetchWithTimeout(`${node.url}/api/accounts`);
      const accounts = data.accounts || [];
      
      const balances = new Map<string, number>();
      for (const acc of accounts) {
        balances.set(acc.address, acc.balance);
      }
      balanceMaps.set(node.name, balances);
      
      console.log(`  ${node.name}: ${accounts.length} accounts`);
    } catch (e: any) {
      console.log(`  ${node.name}: Failed to fetch - ${e.message}`);
    }
  }
  
  if (balanceMaps.size >= 2) {
    const nodeNames = [...balanceMaps.keys()];
    let mismatches = 0;
    
    const allAddresses = new Set<string>();
    for (const balances of balanceMaps.values()) {
      for (const addr of balances.keys()) {
        allAddresses.add(addr);
      }
    }
    
    for (const addr of allAddresses) {
      const balances = nodeNames.map(n => balanceMaps.get(n)?.get(addr) || 0);
      const unique = [...new Set(balances)];
      
      if (unique.length > 1) {
        mismatches++;
        if (mismatches <= 3) {
          console.log(`\n  Balance mismatch for ${addr.slice(0, 12)}...:`);
          for (let i = 0; i < nodeNames.length; i++) {
            console.log(`    ${nodeNames[i]}: ${balances[i]}`);
          }
        }
      }
    }
    
    record('Account balance consistency', mismatches === 0,
      mismatches === 0 
        ? `All ${allAddresses.size} accounts match across nodes`
        : `${mismatches} accounts have balance mismatches`
    );
  }
}

async function validatePeerConnectivity(nodeStatuses: NodeStatus[]) {
  logSection('6. PEER-TO-PEER CONNECTIVITY');
  
  const reachable = nodeStatuses.filter(n => n.reachable);
  
  for (const node of reachable) {
    try {
      const { data } = await fetchWithTimeout(`${node.url}/api/peers`);
      const peers = data.peers || [];
      
      console.log(`  ${node.name}: ${peers.length} peers connected`);
      for (const peer of peers.slice(0, 5)) {
        console.log(`    - ${peer.url || peer.address || peer}`);
      }
      
      record(`${node.name} has peers`, peers.length > 0,
        peers.length > 0 ? `${peers.length} peers` : 'No peers connected'
      );
    } catch (e: any) {
      console.log(`  ${node.name}: Peer info unavailable`);
    }
  }
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
      console.log(`    - ${f.check}: ${f.details}`);
    }
  }
  
  console.log('\n' + '='.repeat(70));
  
  if (failed.length === 0) {
    console.log('\x1b[32m  ALL NODES IN CONSENSUS - NETWORK IS HEALTHY\x1b[0m');
  } else if (failed.length <= 2) {
    console.log('\x1b[33m  MINOR ISSUES DETECTED - LIKELY SYNC LAG\x1b[0m');
  } else {
    console.log('\x1b[31m  SIGNIFICANT ISSUES - INVESTIGATE IMMEDIATELY\x1b[0m');
  }
  
  console.log('='.repeat(70) + '\n');
  
  return failed.length === 0;
}

async function main() {
  console.log('\n');
  console.log('╔══════════════════════════════════════════════════════════════════════╗');
  console.log('║         RINKU MULTI-NODE VALIDATION SCRIPT                           ║');
  console.log('╚══════════════════════════════════════════════════════════════════════╝');
  console.log(`\n  Nodes to validate: ${nodes.length}`);
  for (let i = 0; i < nodes.length; i++) {
    console.log(`    Node ${i + 1}: ${nodes[i]}`);
  }
  console.log(`  Started: ${new Date().toISOString()}`);
  
  try {
    const nodeStatuses = await Promise.all(
      nodes.map((url, index) => getNodeStatus(url, index))
    );
    
    const canProceed = await validateConnectivity(nodeStatuses);
    
    if (!canProceed) {
      console.log('\n\x1b[31m  FATAL: Not enough nodes reachable for comparison\x1b[0m\n');
      process.exit(1);
    }
    
    await validateCheckpointConsensus(nodeStatuses);
    await validateDAGState(nodeStatuses);
    await validateTransactionSync(nodeStatuses);
    await validateAccountBalances(nodeStatuses);
    await validatePeerConnectivity(nodeStatuses);
    
    const success = await generateReport();
    process.exit(success ? 0 : 1);
    
  } catch (e: any) {
    console.error(`\n\x1b[31m  FATAL ERROR: ${e.message}\x1b[0m\n`);
    process.exit(1);
  }
}

main();
