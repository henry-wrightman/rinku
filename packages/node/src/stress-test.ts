import { hashTransaction, type SignedTransaction } from '@rinku/core';

const NODE_URL = process.env.RINKU_NODE_URL || 'http://localhost:3001';
const FAUCET_URL = process.env.RINKU_FAUCET_URL || 'http://localhost:3002';
const TX_COUNT = parseInt(process.env.TX_COUNT || '500', 10);
const BATCH_SIZE = 50;

interface SyncStatus {
  nodeId: string;
  merkleRoot: string;
  dagSize: number;
  tips: string[];
  tipUrls: string[];
}

async function getStatus(url: string): Promise<SyncStatus> {
  const res = await fetch(`${url}/api/sync/status`);
  return res.json() as Promise<SyncStatus>;
}

async function requestFaucet(address: string): Promise<{ txHash: string; amount: number }> {
  const res = await fetch(`${FAUCET_URL}/api/request`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ address })
  });
  
  if (!res.ok) {
    throw new Error(`Faucet request failed: ${res.status}`);
  }
  
  return res.json() as Promise<{ txHash: string; amount: number }>;
}

async function submitTx(tx: SignedTransaction): Promise<{ success: boolean; error?: string }> {
  const res = await fetch(`${NODE_URL}/api/transaction`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(tx)
  });
  return res.json() as Promise<{ success: boolean; error?: string }>;
}

async function getTipUrls(): Promise<string[]> {
  const res = await fetch(`${NODE_URL}/api/tipUrls`);
  const data = await res.json() as { tipUrls: string[] };
  return data.tipUrls;
}

function randomAddress(): string {
  const chars = 'abcdef0123456789';
  let addr = '';
  for (let i = 0; i < 40; i++) {
    addr += chars[Math.floor(Math.random() * chars.length)];
  }
  return addr;
}

async function generateTransactions(count: number): Promise<number> {
  console.log(`\nGenerating ${count} faucet transactions...`);
  console.log('(Each faucet tx creates a new account with 100 coins)\n');
  
  let success = 0;
  let failed = 0;
  const startTime = Date.now();
  
  for (let i = 0; i < count; i++) {
    const addr = randomAddress();
    try {
      await requestFaucet(addr);
      success++;
    } catch (err) {
      failed++;
    }
    
    if ((i + 1) % 10 === 0 || i === count - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(1);
      const tps = success > 0 ? (success / parseFloat(elapsed)).toFixed(1) : '0';
      process.stdout.write(`\r  Progress: ${success + failed}/${count} (${failed} failed) - ${elapsed}s elapsed - ${tps} tx/s`);
    }
  }
  
  console.log('\n');
  return success;
}

async function validateState(): Promise<void> {
  console.log('Validating DAG state...\n');
  
  const status = await getStatus(NODE_URL);
  console.log(`  DAG size: ${status.dagSize} transactions`);
  console.log(`  Active tips: ${status.tips.length}`);
  console.log(`  Tip URLs: ${status.tipUrls.length}`);
  console.log(`  Merkle root: ${status.merkleRoot.slice(0, 16)}...`);
  
  const accountsRes = await fetch(`${NODE_URL}/api/accounts`);
  const accountsData = await accountsRes.json() as { accounts: { fingerprint: string; balance: number }[] };
  
  console.log(`  Accounts: ${accountsData.accounts.length}`);
  
  const totalBalance = accountsData.accounts.reduce((sum, a) => sum + a.balance, 0);
  console.log(`  Total coins: ${totalBalance}`);
}

async function testBootstrapSync(peerUrl: string): Promise<boolean> {
  console.log(`\nTesting bootstrap sync from ${peerUrl}...\n`);
  
  const sourceStatus = await getStatus(peerUrl);
  console.log(`  Source node: ${sourceStatus.dagSize} transactions, root: ${sourceStatus.merkleRoot.slice(0, 16)}...`);
  
  console.log('\n  To test bootstrap, run on another machine:');
  console.log(`    rm -rf .rinku-data`);
  console.log(`    NODE_PORT=3003 NODE_PEERS=${peerUrl} npm run dev:node`);
  console.log('\n  Then verify with:');
  console.log(`    curl http://localhost:3003/api/sync/status`);
  console.log(`\n  Expected: merkleRoot should match ${sourceStatus.merkleRoot.slice(0, 16)}...`);
  
  return true;
}

async function runFullTest(): Promise<void> {
  console.log('='.repeat(60));
  console.log('RINKU STRESS TEST');
  console.log('='.repeat(60));
  console.log(`\nTarget node: ${NODE_URL}`);
  console.log(`Transaction count: ${TX_COUNT}`);
  
  const initialStatus = await getStatus(NODE_URL);
  console.log(`\nInitial state: ${initialStatus.dagSize} transactions`);
  
  const startTime = Date.now();
  const generated = await generateTransactions(TX_COUNT);
  const duration = (Date.now() - startTime) / 1000;
  
  console.log('Generation complete!');
  console.log(`  Time: ${duration.toFixed(1)}s`);
  console.log(`  Successful: ${generated}`);
  console.log(`  Rate: ${(generated / duration).toFixed(1)} tx/s`);
  
  await new Promise(r => setTimeout(r, 2000));
  
  await validateState();
  
  await testBootstrapSync(NODE_URL);
  
  console.log('\n' + '='.repeat(60));
  console.log('STRESS TEST COMPLETE');
  console.log('='.repeat(60) + '\n');
}

runFullTest().catch(console.error);
