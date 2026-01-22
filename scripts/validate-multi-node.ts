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

// Configuration for stress testing
const ACCOUNTS_PER_NODE = 10;  // Number of test accounts to create per node
const PROPAGATION_WAIT_MS = 8000;  // Wait time for gossip propagation

function generateTestAddress(): string {
  return Array.from({ length: 20 }, () => 
    Math.floor(Math.random() * 256).toString(16).padStart(2, '0')
  ).join('');
}

async function testTransactionPropagation(nodeStatuses: NodeStatus[]) {
  logSection('5. TRANSACTION PROPAGATION TEST (Live Consensus)');
  
  const reachable = nodeStatuses.filter(n => n.reachable);
  
  if (reachable.length < 2) {
    console.log('  Skipping: Need at least 2 reachable nodes for propagation test');
    return;
  }
  
  const totalTxTarget = reachable.length * ACCOUNTS_PER_NODE;
  console.log(`  Testing with ${ACCOUNTS_PER_NODE} accounts per node (${totalTxTarget} total transactions)`);
  console.log(`  Nodes under test: ${reachable.map(n => n.name).join(', ')}\n`);
  
  // Generate all test addresses upfront
  const testAccounts: { nodeIndex: number; address: string }[] = [];
  for (let nodeIdx = 0; nodeIdx < reachable.length; nodeIdx++) {
    for (let i = 0; i < ACCOUNTS_PER_NODE; i++) {
      testAccounts.push({
        nodeIndex: nodeIdx,
        address: generateTestAddress()
      });
    }
  }
  
  // Submit faucet requests across all nodes
  const txResults: { 
    node: string; 
    address: string; 
    success: boolean; 
    hash?: string;
    error?: string;
  }[] = [];
  
  console.log('  Phase 1: Submitting transactions...');
  let submitted = 0;
  let failed = 0;
  
  // Submit in batches to avoid overwhelming any single node
  for (let i = 0; i < testAccounts.length; i++) {
    const { nodeIndex, address } = testAccounts[i];
    const node = reachable[nodeIndex];
    
    try {
      const response = await fetch(`${node.url}/api/faucet/request`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ address }),
      });
      
      if (response.ok) {
        const data = await response.json();
        const hash = data.hash || data.txHash || data.transaction?.hash || 'unknown';
        txResults.push({ node: node.name, address, success: true, hash });
        submitted++;
      } else {
        const text = await response.text();
        txResults.push({ node: node.name, address, success: false, error: text.slice(0, 100) });
        failed++;
      }
    } catch (e: any) {
      txResults.push({ node: node.name, address, success: false, error: e.message });
      failed++;
    }
    
    // Progress indicator every 10 transactions
    if ((i + 1) % 10 === 0 || i === testAccounts.length - 1) {
      process.stdout.write(`\r    Submitted: ${submitted} successful, ${failed} failed (${i + 1}/${testAccounts.length})`);
    }
    
    // Small delay between requests to avoid rate limiting
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  console.log('');
  
  const successfulTxs = txResults.filter(r => r.success);
  
  // Report submission stats per node
  console.log('\n  Submission results per node:');
  for (const node of reachable) {
    const nodeTxs = txResults.filter(r => r.node === node.name);
    const nodeSuccess = nodeTxs.filter(r => r.success).length;
    const nodeFailed = nodeTxs.filter(r => !r.success).length;
    console.log(`    ${node.name}: ${nodeSuccess} submitted, ${nodeFailed} failed`);
  }
  
  if (successfulTxs.length === 0) {
    console.log('\n  \x1b[31m✗\x1b[0m No transactions could be submitted');
    
    // Show sample errors
    const sampleErrors = txResults.filter(r => r.error).slice(0, 3);
    if (sampleErrors.length > 0) {
      console.log('  Sample errors:');
      for (const e of sampleErrors) {
        console.log(`    - ${e.node}: ${e.error}`);
      }
    }
    
    record('Transaction submission', false, 'No transactions could be submitted');
    return;
  }
  
  record('Transaction submission', successfulTxs.length >= totalTxTarget * 0.8,
    `${successfulTxs.length}/${totalTxTarget} transactions submitted successfully`
  );
  
  // Wait for propagation
  console.log(`\n  Phase 2: Waiting ${PROPAGATION_WAIT_MS/1000}s for gossip propagation...`);
  await new Promise(resolve => setTimeout(resolve, PROPAGATION_WAIT_MS));
  
  // Verify propagation: check if accounts exist on ALL nodes
  console.log('\n  Phase 3: Verifying cross-node propagation...');
  
  let fullyPropagated = 0;
  let partiallyPropagated = 0;
  let notPropagated = 0;
  const propagationDetails: { address: string; seenOn: number }[] = [];
  
  for (let i = 0; i < successfulTxs.length; i++) {
    const tx = successfulTxs[i];
    let seenOnNodes = 0;
    
    for (const node of reachable) {
      try {
        const { data } = await fetchWithTimeout(`${node.url}/api/accounts/${tx.address}`, 3000);
        if (data && data.balance > 0) {
          seenOnNodes++;
        }
      } catch {
        // Account might not exist yet
      }
    }
    
    propagationDetails.push({ address: tx.address, seenOn: seenOnNodes });
    
    if (seenOnNodes === reachable.length) {
      fullyPropagated++;
    } else if (seenOnNodes > 1) {
      partiallyPropagated++;
    } else {
      notPropagated++;
    }
    
    // Progress indicator
    if ((i + 1) % 10 === 0 || i === successfulTxs.length - 1) {
      process.stdout.write(`\r    Checked: ${i + 1}/${successfulTxs.length} accounts`);
    }
  }
  console.log('');
  
  // Summary
  console.log('\n  Propagation results:');
  console.log(`    \x1b[32m✓\x1b[0m Fully propagated (on all ${reachable.length} nodes): ${fullyPropagated}`);
  if (partiallyPropagated > 0) {
    console.log(`    \x1b[33m⏳\x1b[0m Partially propagated: ${partiallyPropagated}`);
  }
  if (notPropagated > 0) {
    console.log(`    \x1b[31m✗\x1b[0m Not propagated: ${notPropagated}`);
  }
  
  // Calculate propagation rate
  const propagationRate = (fullyPropagated / successfulTxs.length) * 100;
  console.log(`\n  Propagation rate: ${propagationRate.toFixed(1)}%`);
  
  // Show sample of failed propagations
  if (notPropagated > 0 || partiallyPropagated > 0) {
    const failed = propagationDetails.filter(p => p.seenOn < reachable.length).slice(0, 5);
    console.log('  Sample incomplete propagations:');
    for (const f of failed) {
      console.log(`    ${f.address.slice(0, 12)}... seen on ${f.seenOn}/${reachable.length} nodes`);
    }
  }
  
  // Pass if at least 90% of transactions propagated to all nodes
  const passed = propagationRate >= 90;
  record('Transaction propagation', passed,
    passed 
      ? `${propagationRate.toFixed(1)}% of ${successfulTxs.length} transactions propagated to all nodes`
      : `Only ${propagationRate.toFixed(1)}% propagated (need 90%+)`
  );
  
  // Additional consensus check: verify balances match across nodes for propagated accounts
  if (fullyPropagated > 0) {
    console.log('\n  Phase 4: Verifying balance consistency...');
    
    const sampleAccounts = propagationDetails
      .filter(p => p.seenOn === reachable.length)
      .slice(0, 10);
    
    let balanceMismatches = 0;
    for (const acc of sampleAccounts) {
      const balances: number[] = [];
      for (const node of reachable) {
        try {
          const { data } = await fetchWithTimeout(`${node.url}/api/accounts/${acc.address}`, 3000);
          balances.push(data?.balance || 0);
        } catch {
          balances.push(-1);
        }
      }
      
      const uniqueBalances = [...new Set(balances.filter(b => b >= 0))];
      if (uniqueBalances.length > 1) {
        balanceMismatches++;
        console.log(`    \x1b[31m✗\x1b[0m Balance mismatch for ${acc.address.slice(0, 12)}...: ${balances.join(' vs ')}`);
      }
    }
    
    if (balanceMismatches === 0) {
      console.log(`    \x1b[32m✓\x1b[0m All ${sampleAccounts.length} sampled accounts have consistent balances`);
    }
    
    record('Balance consistency', balanceMismatches === 0,
      balanceMismatches === 0 
        ? `All sampled accounts have matching balances across nodes`
        : `${balanceMismatches} accounts have balance mismatches`
    );
  }
}

type AccountRow = {
  fingerprint: string;
  balance: number;
  nonce: number;
  staked: number;
};

type AccountSnapshot = {
  balance: number;
  nonce: number;
  staked: number;
};

function nearlyEqual(a: number, b: number, eps = 1e-9): boolean {
  // absolute tolerance; tweak as needed
  return Math.abs(a - b) <= eps;
}

function fmt(n: number): string {
  // nicer printing for floats
  return Number.isFinite(n) ? n.toFixed(10).replace(/\.?0+$/, "") : String(n);
}

async function validateAccountBalances(nodeStatuses: NodeStatus[]) {
  logSection("6. ACCOUNT STATE CONSISTENCY");

  const reachable = nodeStatuses.filter(n => n.reachable);
  const snapshotsByNode = new Map<string, Map<string, AccountSnapshot>>();

  for (const node of reachable) {
    try {
      const { data } = await fetchWithTimeout(`${node.url}/api/accounts`);
      const accounts: AccountRow[] = data.accounts || [];

      const m = new Map<string, AccountSnapshot>();
      for (const acc of accounts) {
        if (!acc?.fingerprint) continue;
        m.set(acc.fingerprint, {
          balance: Number(acc.balance ?? 0),
          nonce: Number(acc.nonce ?? 0),
          staked: Number(acc.staked ?? 0),
        });
      }

      snapshotsByNode.set(node.name, m);
      console.log(`  ${node.name}: ${accounts.length} accounts`);
    } catch (e: any) {
      console.log(`  ${node.name}: Failed to fetch - ${e.message}`);
    }
  }

  if (snapshotsByNode.size < 2) {
    record("Account state consistency", true, "Not enough reachable nodes to compare");
    return;
  }

  const nodeNames = [...snapshotsByNode.keys()];

  // union of all fingerprints
  const allFingerprints = new Set<string>();
  for (const m of snapshotsByNode.values()) {
    for (const fp of m.keys()) allFingerprints.add(fp);
  }

  let mismatchAccounts = 0;
  let missingAccounts = 0;

  // how many mismatches to print
  const MAX_PRINT = 5;

  for (const fp of allFingerprints) {
    const perNode = nodeNames.map(n => snapshotsByNode.get(n)!.get(fp) ?? null);

    // detect missing on some nodes
    const presentCount = perNode.filter(x => x !== null).length;
    if (presentCount !== perNode.length) {
      missingAccounts++;
      if (missingAccounts <= MAX_PRINT) {
        console.log(`\n  Account missing on some nodes: ${fp}`);
        for (let i = 0; i < nodeNames.length; i++) {
          console.log(`    ${nodeNames[i]}: ${perNode[i] ? "present" : "MISSING"}`);
        }
      }
      continue; // optional: skip value comparison if missing
    }

    const vals = perNode as AccountSnapshot[];

    // choose node 0 as reference
    const ref = vals[0];

    const balanceMismatch = vals.some(v => !nearlyEqual(v.balance, ref.balance, 1e-8));
    const stakeMismatch   = vals.some(v => !nearlyEqual(v.staked, ref.staked, 1e-8));
    const nonceMismatch   = vals.some(v => v.nonce !== ref.nonce);

    if (balanceMismatch || stakeMismatch || nonceMismatch) {
      mismatchAccounts++;

      if (mismatchAccounts <= MAX_PRINT) {
        console.log(`\n  State mismatch for ${fp}:`);
        for (let i = 0; i < nodeNames.length; i++) {
          const v = vals[i];
          console.log(
            `    ${nodeNames[i]}: balance=${fmt(v.balance)} nonce=${v.nonce} staked=${fmt(v.staked)}`
          );
        }
      }
    }
  }

  const ok = mismatchAccounts === 0 && missingAccounts === 0;
  record(
    "Account state consistency",
    ok,
    ok
      ? `All ${allFingerprints.size} accounts match across ${nodeNames.length} nodes`
      : `${mismatchAccounts} accounts mismatched, ${missingAccounts} missing`
  );
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
    // await testTransactionPropagation(nodeStatuses);
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
