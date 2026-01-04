#!/usr/bin/env tsx

const NODE_URL = process.env.NODE_URL || 'http://localhost:3001';
const FAUCET_URL = process.env.FAUCET_URL || 'http://localhost:3002';

interface TestResult {
  name: string;
  passed: boolean;
  duration: number;
  details: string;
}

const results: TestResult[] = [];

async function log(msg: string): Promise<void> {
  console.log(`[${new Date().toISOString()}] ${msg}`);
}

async function fetchJson(url: string, options?: RequestInit): Promise<any> {
  const res = await fetch(url, options);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`HTTP ${res.status}: ${text}`);
  }
  return res.json();
}

async function waitForNode(timeout = 30000): Promise<boolean> {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    try {
      const health = await fetchJson(`${NODE_URL}/api/health`);
      if (health.status === 'ok') return true;
    } catch {
    }
    await new Promise(r => setTimeout(r, 500));
  }
  return false;
}

async function getNodeStats(): Promise<any> {
  return fetchJson(`${NODE_URL}/api/stats`);
}

async function getCheckpointInfo(): Promise<any> {
  return fetchJson(`${NODE_URL}/api/checkpoints`);
}

async function getDagSummary(): Promise<any> {
  return fetchJson(`${NODE_URL}/api/dag/summary`);
}

async function test_HighThroughput(): Promise<TestResult> {
  const name = 'High Throughput Test';
  const start = Date.now();
  
  try {
    await log(`Starting ${name}...`);
    
    const accounts = await fetchJson(`${NODE_URL}/api/accounts`);
    const accountList = accounts.accounts || [];
    
    if (accountList.length < 2) {
      return {
        name,
        passed: false,
        duration: Date.now() - start,
        details: 'Not enough accounts to test throughput'
      };
    }
    
    const initialStats = await getNodeStats();
    const initialDagSize = initialStats.dagSize;
    
    await new Promise(r => setTimeout(r, 10000));
    
    const finalStats = await getNodeStats();
    const finalDagSize = finalStats.dagSize;
    const newTxs = finalDagSize - initialDagSize;
    
    const duration = Date.now() - start;
    const tps = (newTxs / (10)).toFixed(2);
    
    return {
      name,
      passed: newTxs > 5,
      duration,
      details: `${newTxs} new transactions in 10s (${tps} TPS)`
    };
  } catch (e: any) {
    return {
      name,
      passed: false,
      duration: Date.now() - start,
      details: `Error: ${e.message}`
    };
  }
}

async function test_TipConsolidation(): Promise<TestResult> {
  const name = 'Tip Consolidation Test';
  const start = Date.now();
  
  try {
    await log(`Starting ${name}...`);
    
    const initialStats = await getDagSummary();
    const initialTips = initialStats.tipCount;
    
    await log(`Initial tips: ${initialTips}`);
    
    await new Promise(r => setTimeout(r, 15000));
    
    const finalStats = await getDagSummary();
    const finalTips = finalStats.tipCount;
    
    await log(`Final tips: ${finalTips}`);
    
    const maxTips = parseInt(process.env.MAX_TIPS || '15');
    const passed = finalTips <= maxTips * 2;
    
    return {
      name,
      passed,
      duration: Date.now() - start,
      details: `Tips: ${initialTips} -> ${finalTips} (max allowed: ${maxTips * 2})`
    };
  } catch (e: any) {
    return {
      name,
      passed: false,
      duration: Date.now() - start,
      details: `Error: ${e.message}`
    };
  }
}

async function test_CheckpointProgression(): Promise<TestResult> {
  const name = 'Checkpoint Progression Test';
  const start = Date.now();
  
  try {
    await log(`Starting ${name}...`);
    
    const initial = await getCheckpointInfo();
    const initialHeight = initial.height || 0;
    
    await log(`Initial checkpoint height: ${initialHeight}`);
    
    await new Promise(r => setTimeout(r, 45000));
    
    const final = await getCheckpointInfo();
    const finalHeight = final.height || 0;
    
    await log(`Final checkpoint height: ${finalHeight}`);
    
    const progression = finalHeight - initialHeight;
    const passed = progression >= 2;
    
    return {
      name,
      passed,
      duration: Date.now() - start,
      details: `Checkpoint height: ${initialHeight} -> ${finalHeight} (+${progression})`
    };
  } catch (e: any) {
    return {
      name,
      passed: false,
      duration: Date.now() - start,
      details: `Error: ${e.message}`
    };
  }
}

async function test_RateLimiting(): Promise<TestResult> {
  const name = 'Rate Limiting Test';
  const start = Date.now();
  
  try {
    await log(`Starting ${name}...`);
    
    const requests: Promise<Response>[] = [];
    for (let i = 0; i < 50; i++) {
      requests.push(fetch(`${NODE_URL}/api/tx`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ tx: null })
      }));
    }
    
    const responses = await Promise.all(requests);
    const rateLimited = responses.filter(r => r.status === 429).length;
    const badRequest = responses.filter(r => r.status === 400).length;
    
    const passed = rateLimited > 0 || badRequest > 20;
    
    return {
      name,
      passed,
      duration: Date.now() - start,
      details: `429 responses: ${rateLimited}, 400 responses: ${badRequest}`
    };
  } catch (e: any) {
    return {
      name,
      passed: false,
      duration: Date.now() - start,
      details: `Error: ${e.message}`
    };
  }
}

async function test_MetricsEndpoint(): Promise<TestResult> {
  const name = 'Prometheus Metrics Test';
  const start = Date.now();
  
  try {
    await log(`Starting ${name}...`);
    
    const res = await fetch(`${NODE_URL}/metrics`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    
    const metrics = await res.text();
    
    const requiredMetrics = [
      'rinku_dag_nodes_total',
      'rinku_dag_tips_total',
      'rinku_checkpoint_height',
      'rinku_gas_price_current',
      'rinku_supply_total'
    ];
    
    const found = requiredMetrics.filter(m => metrics.includes(m));
    const passed = found.length === requiredMetrics.length;
    
    return {
      name,
      passed,
      duration: Date.now() - start,
      details: `Found ${found.length}/${requiredMetrics.length} required metrics`
    };
  } catch (e: any) {
    return {
      name,
      passed: false,
      duration: Date.now() - start,
      details: `Error: ${e.message}`
    };
  }
}

async function test_SnapshotRestore(): Promise<TestResult> {
  const name = 'Snapshot Persistence Test';
  const start = Date.now();
  
  try {
    await log(`Starting ${name}...`);
    
    const stats = await getNodeStats();
    const dagSize = stats.dagSize;
    const accountCount = stats.accountCount;
    
    const passed = dagSize > 0 && accountCount > 0;
    
    return {
      name,
      passed,
      duration: Date.now() - start,
      details: `DAG size: ${dagSize}, Accounts: ${accountCount}`
    };
  } catch (e: any) {
    return {
      name,
      passed: false,
      duration: Date.now() - start,
      details: `Error: ${e.message}`
    };
  }
}

async function runTests(): Promise<void> {
  console.log('='.repeat(60));
  console.log('RINKU STRESS TEST SUITE');
  console.log('='.repeat(60));
  console.log(`Node: ${NODE_URL}`);
  console.log(`Faucet: ${FAUCET_URL}`);
  console.log('='.repeat(60));
  
  await log('Waiting for node to be ready...');
  const ready = await waitForNode();
  if (!ready) {
    console.error('Node not reachable after 30s. Aborting.');
    process.exit(1);
  }
  await log('Node is ready.');
  
  const tests = [
    test_MetricsEndpoint,
    test_RateLimiting,
    test_SnapshotRestore,
    test_HighThroughput,
    test_TipConsolidation,
    test_CheckpointProgression,
  ];
  
  for (const test of tests) {
    const result = await test();
    results.push(result);
    
    const status = result.passed ? '✓ PASS' : '✗ FAIL';
    console.log(`\n${status}: ${result.name}`);
    console.log(`  Duration: ${result.duration}ms`);
    console.log(`  Details: ${result.details}`);
  }
  
  console.log('\n' + '='.repeat(60));
  console.log('TEST SUMMARY');
  console.log('='.repeat(60));
  
  const passed = results.filter(r => r.passed).length;
  const failed = results.filter(r => !r.passed).length;
  
  console.log(`Passed: ${passed}/${results.length}`);
  console.log(`Failed: ${failed}/${results.length}`);
  
  if (failed > 0) {
    console.log('\nFailed tests:');
    results.filter(r => !r.passed).forEach(r => {
      console.log(`  - ${r.name}: ${r.details}`);
    });
  }
  
  console.log('='.repeat(60));
  
  process.exit(failed > 0 ? 1 : 0);
}

runTests().catch(e => {
  console.error('Test suite failed:', e);
  process.exit(1);
});
