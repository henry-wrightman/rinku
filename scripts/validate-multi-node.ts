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
      const chain = cpData.checkpoints || cpData.chain || [];
      if (chain.length > 0) {
        const latest = chain[0]; // First is latest (sorted descending)
        checkpointHeight = latest.height || latest.checkpointHeight;
        checkpointId = latest.hash || latest.checkpointId;
        txMerkleRoot = latest.merkleRoot || latest.txMerkleRoot;
      }
    } catch {}
    
    // Tips can be: array of hashes, comma-separated string, or tipCount field
    let tipCount = 0;
    if (typeof status.tipCount === 'number') {
      tipCount = status.tipCount;
    } else if (Array.isArray(status.tips)) {
      tipCount = status.tips.length;
    } else if (typeof status.tips === 'string') {
      tipCount = status.tips.split(',').filter((t: string) => t.length > 0).length;
    } else if (typeof status.tips === 'number') {
      tipCount = status.tips;
    }
    
    return {
      url,
      name,
      reachable: true,
      merkleRoot: status.merkleRoot,
      dagSize: status.dagSize,
      tips: tipCount,
      // Use checkpoint height from sync/status if available, fallback to checkpoints endpoint
      checkpointHeight: status.checkpointHeight ?? checkpointHeight,
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
  logSection('2. CHECKPOINT CONSENSUS (Critical for consensus)');
  
  const reachable = nodeStatuses.filter(n => n.reachable);
  
  const heights = reachable.map(n => n.checkpointHeight).filter(h => h !== undefined) as number[];
  
  if (heights.length === 0) {
    record('Checkpoint height consensus', true, 'No checkpoints on any node');
    return;
  }
  
  const maxHeight = Math.max(...heights);
  const minHeight = Math.min(...heights);
  const heightDiff = maxHeight - minHeight;
  
  // Height sync is informational - nodes create checkpoints at different times
  console.log(`  Checkpoint heights: ${minHeight}-${maxHeight} (diff: ${heightDiff})`);
  record('Checkpoint height sync', heightDiff <= 5, 
    heightDiff <= 5
      ? `All nodes within 5 checkpoints (acceptable sync lag)`
      : `Large gap: ${heightDiff} checkpoints - check sync`
  );
  
  // THE CRITICAL CHECK: Compare merkle roots at a COMMON checkpoint height
  console.log(`\n  Comparing checkpoint merkle roots at common height ${minHeight}...`);
  
  const checkpointRootsAtCommon = new Map<string, { root: string; hash: string }>();
  
  for (const node of reachable) {
    try {
      const { data } = await fetchWithTimeout(`${node.url}/api/checkpoints`);
      const checkpoints = data.checkpoints || data.chain || [];
      
      // Find checkpoint at minHeight (common to all nodes)
      const cp = checkpoints.find((c: any) => (c.height || c.checkpointHeight) === minHeight);
      if (cp) {
        checkpointRootsAtCommon.set(node.name, {
          root: cp.merkleRoot || cp.txMerkleRoot || '',
          hash: cp.hash || cp.checkpointId || ''
        });
        console.log(`    ${node.name} @ height ${minHeight}: ${(cp.merkleRoot || cp.txMerkleRoot || 'N/A').slice(0, 16)}...`);
      } else {
        console.log(`    ${node.name}: Checkpoint ${minHeight} not found`);
      }
    } catch (e: any) {
      console.log(`    ${node.name}: Failed to fetch checkpoints - ${e.message}`);
    }
  }
  
  if (checkpointRootsAtCommon.size >= 2) {
    const roots = [...checkpointRootsAtCommon.values()].map(v => v.root);
    const uniqueRoots = [...new Set(roots)];
    
    record('Checkpoint merkle root consensus', uniqueRoots.length === 1,
      uniqueRoots.length === 1
        ? `All nodes agree at height ${minHeight}: ${uniqueRoots[0]?.slice(0, 16)}...`
        : `FORK DETECTED at height ${minHeight}: ${uniqueRoots.length} different roots!`
    );
    
    const hashes = [...checkpointRootsAtCommon.values()].map(v => v.hash);
    const uniqueHashes = [...new Set(hashes)];
    
    if (uniqueRoots.length === 1) {
      record('Checkpoint hash consensus', uniqueHashes.length === 1,
        uniqueHashes.length === 1
          ? `All nodes agree: ${uniqueHashes[0]?.slice(0, 16)}...`
          : `Hash mismatch (possible different validators signing)`
      );
    }
  }
}

async function validateDAGState(nodeStatuses: NodeStatus[]) {
  logSection('3. DAG STATE COMPARISON (Informational - varies by pruning)');
  
  const reachable = nodeStatuses.filter(n => n.reachable);
  
  const dagSizes = reachable.map(n => ({ name: n.name, size: n.dagSize || 0 }));
  const maxSize = Math.max(...dagSizes.map(d => d.size));
  const minSize = Math.min(...dagSizes.map(d => d.size));
  
  console.log('\n  DAG Sizes (unfinalized transactions - expected to vary):');
  for (const { name, size } of dagSizes) {
    console.log(`    ${name}: ${size} transactions`);
  }
  
  const sizeDiffPercent = maxSize > 0 ? ((maxSize - minSize) / maxSize) * 100 : 0;
  // DAG size is informational only - nodes prune at different times
  console.log(`  \x1b[33mℹ\x1b[0m DAG size variance: ${sizeDiffPercent.toFixed(1)}% (${minSize} - ${maxSize} txs) - Expected due to pruning`);
  
  const merkleRoots = reachable.map(n => n.merkleRoot).filter(Boolean);
  const uniqueRoots = [...new Set(merkleRoots)];
  
  if (merkleRoots.length > 1) {
    // DAG merkle root is informational - changes with each transaction
    console.log(`  \x1b[33mℹ\x1b[0m DAG merkle roots: ${uniqueRoots.length} different (expected - DAG changes constantly)`);
  }
  
  console.log('\n  Tip Counts:');
  for (const node of reachable) {
    console.log(`    ${node.name}: ${node.tips || 0} tips`);
  }
}

async function validateTransactionSync(nodeStatuses: NodeStatus[]) {
  logSection('4. DAG TRANSACTION COUNTS (Informational - varies by pruning)');
  
  const reachable = nodeStatuses.filter(n => n.reachable);
  
  console.log('  Note: DAG transactions differ due to pruning after checkpoints.');
  console.log('  This is expected behavior with snapshot-based sync.\n');
  
  for (const node of reachable) {
    try {
      // Just get first page to show approximate count
      const { data } = await fetchWithTimeout(`${node.url}/api/dag?page=1&limit=50`, 10000);
      const transactions = data.transactions || data.nodes || data || [];
      const dagSize = node.dagSize || transactions.length;
      console.log(`  ${node.name}: ~${dagSize} DAG transactions`);
    } catch (e: any) {
      console.log(`  ${node.name}: Failed to fetch - ${e.message}`);
    }
  }
  
  console.log(`\n  \x1b[33mℹ\x1b[0m DAG transaction differences are expected - transactions are pruned after finalization.`);
}

async function testTransactionPropagation(nodeStatuses: NodeStatus[]) {
  logSection('5. TRANSACTION PROPAGATION TEST (Live Consensus)');
  
  const reachable = nodeStatuses.filter(n => n.reachable);
  
  if (reachable.length < 2) {
    console.log('  Skipping: Need at least 2 reachable nodes for propagation test');
    return;
  }
  
  console.log('  Creating test wallets and transactions on different nodes...\n');
  
  // Generate test addresses (simple random hex)
  const testAddresses: string[] = [];
  for (let i = 0; i < reachable.length; i++) {
    const addr = Array.from({ length: 20 }, () => 
      Math.floor(Math.random() * 256).toString(16).padStart(2, '0')
    ).join('');
    testAddresses.push(addr);
  }
  
  // Submit a faucet request on each node to its unique test address
  const txResults: { node: string; address: string; success: boolean; hash?: string }[] = [];
  
  for (let i = 0; i < reachable.length; i++) {
    const node = reachable[i];
    const testAddr = testAddresses[i];
    
    try {
      console.log(`  Submitting faucet tx on ${node.name} to ${testAddr.slice(0, 12)}...`);
      
      const response = await fetch(`${node.url}/api/faucet`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ address: testAddr }),
      });
      
      if (response.ok) {
        const data = await response.json();
        const hash = data.hash || data.txHash || data.transaction?.hash || 'unknown';
        console.log(`    ✓ Transaction submitted: ${hash.slice(0, 16)}...`);
        txResults.push({ node: node.name, address: testAddr, success: true, hash });
      } else {
        const text = await response.text();
        console.log(`    ✗ Failed: ${text.slice(0, 50)}`);
        txResults.push({ node: node.name, address: testAddr, success: false });
      }
    } catch (e: any) {
      console.log(`    ✗ Error: ${e.message}`);
      txResults.push({ node: node.name, address: testAddr, success: false });
    }
  }
  
  const successfulTxs = txResults.filter(r => r.success);
  
  if (successfulTxs.length === 0) {
    console.log('\n  No transactions could be submitted - skipping propagation check');
    console.log('  (This may be normal if faucet is disabled or rate-limited)');
    return;
  }
  
  // Wait for propagation
  console.log(`\n  Waiting 5 seconds for transaction propagation...`);
  await new Promise(resolve => setTimeout(resolve, 5000));
  
  // Check if test addresses have balances on all nodes
  console.log('\n  Verifying transaction propagation:');
  
  let propagationSuccess = 0;
  let propagationTotal = 0;
  
  for (const tx of successfulTxs) {
    propagationTotal++;
    let seenOnNodes = 0;
    
    for (const node of reachable) {
      try {
        const { data } = await fetchWithTimeout(`${node.url}/api/accounts/${tx.address}`, 5000);
        if (data && data.balance > 0) {
          seenOnNodes++;
        }
      } catch {
        // Account might not exist yet
      }
    }
    
    const propagated = seenOnNodes === reachable.length;
    if (propagated) propagationSuccess++;
    
    const status = propagated ? '\x1b[32m✓\x1b[0m' : '\x1b[33m⏳\x1b[0m';
    console.log(`    ${status} Tx from ${tx.node}: seen on ${seenOnNodes}/${reachable.length} nodes`);
  }
  
  if (propagationTotal > 0) {
    const allPropagated = propagationSuccess === propagationTotal;
    record('Transaction propagation', allPropagated,
      allPropagated 
        ? `All ${propagationTotal} test transactions propagated to all nodes`
        : `${propagationSuccess}/${propagationTotal} transactions fully propagated (others may still be syncing)`
    );
  }
}

async function validateAccountBalances(nodeStatuses: NodeStatus[]) {
  logSection('6. ACCOUNT BALANCE CONSISTENCY');
  
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
      if (!addr) continue;
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
  logSection('7. PEER-TO-PEER CONNECTIVITY');
  
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
  
  // Identify critical failures (checkpoint consensus related)
  const criticalFailures = failed.filter(f => 
    f.check.includes('merkle root') || 
    f.check.includes('balance') ||
    f.check.includes('FORK')
  );
  
  console.log(`\n  Total Checks: ${totalChecks}`);
  console.log(`  \x1b[32mPassed: ${passedChecks}\x1b[0m`);
  console.log(`  \x1b[31mFailed: ${totalChecks - passedChecks}\x1b[0m`);
  console.log(`  Success Rate: ${((passedChecks / totalChecks) * 100).toFixed(1)}%`);
  
  if (failed.length > 0) {
    console.log('\n  Failed Checks:');
    for (const f of failed) {
      const isCritical = criticalFailures.includes(f);
      const prefix = isCritical ? '\x1b[31m[CRITICAL]\x1b[0m' : '\x1b[33m[MINOR]\x1b[0m';
      console.log(`    ${prefix} ${f.check}: ${f.details}`);
    }
  }
  
  console.log('\n' + '='.repeat(70));
  console.log('  KEY CONSENSUS METRICS:');
  console.log('    - Checkpoint merkle roots at same height MUST match');
  console.log('    - Account balances MUST match');
  console.log('    - DAG size/transactions will differ (expected due to pruning)');
  console.log('='.repeat(70));
  
  if (criticalFailures.length > 0) {
    console.log('\x1b[31m  CONSENSUS FAILURE - FORK OR BALANCE MISMATCH DETECTED\x1b[0m');
  } else if (failed.length === 0) {
    console.log('\x1b[32m  ALL NODES IN CONSENSUS - NETWORK IS HEALTHY\x1b[0m');
  } else {
    console.log('\x1b[33m  MINOR SYNC LAG DETECTED - NODES CONVERGING\x1b[0m');
  }
  
  console.log('='.repeat(70) + '\n');
  
  return criticalFailures.length === 0;
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
    await testTransactionPropagation(nodeStatuses);
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
