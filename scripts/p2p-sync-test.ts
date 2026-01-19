#!/usr/bin/env npx tsx
import { webcrypto } from 'crypto';
import { p256 } from '@noble/curves/p256';
import { sha256 } from '@noble/hashes/sha256';
import { bytesToHex } from '@noble/hashes/utils';

const NODE_1_URL = process.env.NODE_1_URL || 'https://rinkuchan.com';
const NODE_2_URL = process.env.NODE_2_URL || 'https://rinku-node-0.fly.dev';
const TEST_DURATION_MS = parseInt(process.env.TEST_DURATION || '120000');
const POLL_INTERVAL_MS = parseInt(process.env.POLL_INTERVAL || '2000');

interface TestWallet {
  privateKey: Uint8Array;
  publicKey: Uint8Array;
  address: string;
  createdOn: string;
  createdAt: number;
  fundedAt?: number;
  lastSeenNode1?: number;
  lastSeenNode2?: number;
  disappeared: boolean;
  disappearedAt?: number;
  reappeared: boolean;
  reappearedAt?: number;
  balanceNode1?: number;
  balanceNode2?: number;
  nonceNode1?: number;
  nonceNode2?: number;
}

interface SyncEvent {
  timestamp: number;
  type: 'wallet_created' | 'wallet_funded' | 'wallet_seen' | 'wallet_disappeared' | 'wallet_reappeared' | 
        'sync_triggered' | 'checkpoint_mismatch' | 'nonce_mismatch' | 'balance_mismatch' | 'account_count_diff';
  node: string;
  details: Record<string, unknown>;
}

const testWallets: Map<string, TestWallet> = new Map();
const events: SyncEvent[] = [];
let running = true;

function log(msg: string) {
  const time = new Date().toISOString().split('T')[1].slice(0, 12);
  console.log(`[${time}] ${msg}`);
}

function logEvent(event: SyncEvent) {
  events.push(event);
  const time = new Date(event.timestamp).toISOString().split('T')[1].slice(0, 12);
  console.log(`[${time}] EVENT: ${event.type} on ${event.node} - ${JSON.stringify(event.details)}`);
}

function generateWallet(): { privateKey: Uint8Array; publicKey: Uint8Array; address: string } {
  const privateKey = webcrypto.getRandomValues(new Uint8Array(32));
  const publicKey = p256.getPublicKey(privateKey, false);
  const hash = sha256(publicKey);
  const address = bytesToHex(hash).slice(0, 16);
  return { privateKey, publicKey, address };
}

function signTransaction(tx: Record<string, unknown>, privateKey: Uint8Array): string {
  const message = JSON.stringify(tx);
  const messageHash = sha256(new TextEncoder().encode(message));
  const signature = p256.sign(messageHash, privateKey);
  return bytesToHex(signature.toCompactRawBytes());
}

async function fetchWithTimeout(url: string, options: RequestInit = {}, timeout = 10000): Promise<Response> {
  const controller = new AbortController();
  const id = setTimeout(() => controller.abort(), timeout);
  try {
    const response = await fetch(url, { ...options, signal: controller.signal });
    clearTimeout(id);
    return response;
  } catch (e) {
    clearTimeout(id);
    throw e;
  }
}

async function getAccounts(nodeUrl: string): Promise<{ fingerprint: string; balance: number; nonce: number; staked: number }[]> {
  try {
    const resp = await fetchWithTimeout(`${nodeUrl}/api/accounts`);
    if (!resp.ok) return [];
    const data = await resp.json();
    return data.accounts || [];
  } catch {
    return [];
  }
}

async function getNetworkStats(nodeUrl: string): Promise<Record<string, unknown> | null> {
  try {
    const resp = await fetchWithTimeout(`${nodeUrl}/api/stats/network`);
    if (!resp.ok) return null;
    return await resp.json();
  } catch {
    return null;
  }
}

async function getCheckpoints(nodeUrl: string): Promise<{ height: number; merkle_root: string }[]> {
  try {
    const resp = await fetchWithTimeout(`${nodeUrl}/api/checkpoints?limit=5`);
    if (!resp.ok) return [];
    const data = await resp.json();
    return data.checkpoints || [];
  } catch {
    return [];
  }
}

async function fundWallet(nodeUrl: string, address: string): Promise<boolean> {
  try {
    const resp = await fetchWithTimeout(`${nodeUrl}/faucet`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ address, amount: 10 })
    });
    return resp.ok;
  } catch {
    return false;
  }
}

async function sendTransaction(
  nodeUrl: string, 
  from: string, 
  to: string, 
  amount: number, 
  nonce: number,
  privateKey: Uint8Array
): Promise<{ success: boolean; error?: string }> {
  const tx = {
    from,
    to,
    amount,
    nonce,
    timestamp: Date.now(),
    kind: 'transfer'
  };
  const signature = signTransaction(tx, privateKey);
  
  try {
    const resp = await fetchWithTimeout(`${nodeUrl}/api/tx`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ...tx, signature })
    });
    if (!resp.ok) {
      const data = await resp.json().catch(() => ({}));
      return { success: false, error: data.error || `HTTP ${resp.status}` };
    }
    return { success: true };
  } catch (e) {
    return { success: false, error: String(e) };
  }
}

async function createAndFundWallet(targetNode: string): Promise<TestWallet | null> {
  const { privateKey, publicKey, address } = generateWallet();
  const nodeUrl = targetNode === 'node1' ? NODE_1_URL : NODE_2_URL;
  
  const wallet: TestWallet = {
    privateKey,
    publicKey,
    address,
    createdOn: targetNode,
    createdAt: Date.now(),
    disappeared: false,
    reappeared: false
  };
  
  logEvent({
    timestamp: Date.now(),
    type: 'wallet_created',
    node: targetNode,
    details: { address }
  });
  
  const funded = await fundWallet(nodeUrl, address);
  if (funded) {
    wallet.fundedAt = Date.now();
    logEvent({
      timestamp: Date.now(),
      type: 'wallet_funded',
      node: targetNode,
      details: { address }
    });
    testWallets.set(address, wallet);
    return wallet;
  }
  
  log(`Failed to fund wallet ${address} on ${targetNode}`);
  return null;
}

async function checkWalletPresence(): Promise<void> {
  const [accounts1, accounts2] = await Promise.all([
    getAccounts(NODE_1_URL),
    getAccounts(NODE_2_URL)
  ]);
  
  const accounts1Map = new Map(accounts1.map(a => [a.fingerprint, a]));
  const accounts2Map = new Map(accounts2.map(a => [a.fingerprint, a]));
  
  const now = Date.now();
  
  for (const [address, wallet] of testWallets) {
    const onNode1 = accounts1Map.get(address);
    const onNode2 = accounts2Map.get(address);
    
    if (onNode1) {
      wallet.lastSeenNode1 = now;
      wallet.balanceNode1 = onNode1.balance;
      wallet.nonceNode1 = onNode1.nonce;
    }
    
    if (onNode2) {
      wallet.lastSeenNode2 = now;
      wallet.balanceNode2 = onNode2.balance;
      wallet.nonceNode2 = onNode2.nonce;
    }
    
    const wasVisible = wallet.lastSeenNode1 || wallet.lastSeenNode2;
    const isVisible = onNode1 || onNode2;
    
    if (wasVisible && !isVisible && !wallet.disappeared) {
      wallet.disappeared = true;
      wallet.disappearedAt = now;
      logEvent({
        timestamp: now,
        type: 'wallet_disappeared',
        node: 'both',
        details: { 
          address, 
          createdOn: wallet.createdOn,
          createdAt: wallet.createdAt,
          lastSeenNode1: wallet.lastSeenNode1,
          lastSeenNode2: wallet.lastSeenNode2,
          timeSinceCreation: now - wallet.createdAt
        }
      });
    }
    
    if (wallet.disappeared && isVisible && !wallet.reappeared) {
      wallet.reappeared = true;
      wallet.reappearedAt = now;
      logEvent({
        timestamp: now,
        type: 'wallet_reappeared',
        node: onNode1 ? 'node1' : 'node2',
        details: { 
          address,
          disappearedFor: now - (wallet.disappearedAt || 0)
        }
      });
    }
    
    if (onNode1 && onNode2) {
      if (onNode1.nonce !== onNode2.nonce) {
        logEvent({
          timestamp: now,
          type: 'nonce_mismatch',
          node: 'both',
          details: { 
            address, 
            node1Nonce: onNode1.nonce, 
            node2Nonce: onNode2.nonce,
            diff: Math.abs(onNode1.nonce - onNode2.nonce)
          }
        });
      }
      
      if (Math.abs(onNode1.balance - onNode2.balance) > 0.001) {
        logEvent({
          timestamp: now,
          type: 'balance_mismatch',
          node: 'both',
          details: { 
            address, 
            node1Balance: onNode1.balance, 
            node2Balance: onNode2.balance,
            diff: Math.abs(onNode1.balance - onNode2.balance)
          }
        });
      }
    }
  }
  
  if (accounts1.length !== accounts2.length) {
    logEvent({
      timestamp: now,
      type: 'account_count_diff',
      node: 'both',
      details: { 
        node1Count: accounts1.length, 
        node2Count: accounts2.length,
        diff: Math.abs(accounts1.length - accounts2.length)
      }
    });
  }
}

async function checkCheckpointSync(): Promise<void> {
  const [cp1, cp2] = await Promise.all([
    getCheckpoints(NODE_1_URL),
    getCheckpoints(NODE_2_URL)
  ]);
  
  if (cp1.length === 0 || cp2.length === 0) return;
  
  const latestHeight = Math.max(cp1[0]?.height || 0, cp2[0]?.height || 0);
  
  for (const checkpoint1 of cp1) {
    const checkpoint2 = cp2.find(c => c.height === checkpoint1.height);
    if (checkpoint2 && checkpoint1.merkle_root !== checkpoint2.merkle_root) {
      logEvent({
        timestamp: Date.now(),
        type: 'checkpoint_mismatch',
        node: 'both',
        details: {
          height: checkpoint1.height,
          node1Root: checkpoint1.merkle_root.slice(0, 16),
          node2Root: checkpoint2.merkle_root.slice(0, 16)
        }
      });
    }
  }
}

async function runTest(): Promise<void> {
  console.log('='.repeat(70));
  console.log('P2P SYNC INTEGRITY TEST');
  console.log('='.repeat(70));
  console.log(`Node 1: ${NODE_1_URL}`);
  console.log(`Node 2: ${NODE_2_URL}`);
  console.log(`Duration: ${TEST_DURATION_MS / 1000}s`);
  console.log(`Poll interval: ${POLL_INTERVAL_MS}ms`);
  console.log('='.repeat(70));
  
  const [stats1, stats2] = await Promise.all([
    getNetworkStats(NODE_1_URL),
    getNetworkStats(NODE_2_URL)
  ]);
  
  console.log('\nInitial Node States:');
  console.log(`Node 1: ${JSON.stringify(stats1, null, 2)}`);
  console.log(`Node 2: ${JSON.stringify(stats2, null, 2)}`);
  console.log('');
  
  const startTime = Date.now();
  let walletsCreated = 0;
  let lastWalletCreation = 0;
  
  const monitorLoop = async () => {
    while (running && Date.now() - startTime < TEST_DURATION_MS) {
      await checkWalletPresence();
      await checkCheckpointSync();
      await new Promise(r => setTimeout(r, POLL_INTERVAL_MS));
    }
  };
  
  const walletCreationLoop = async () => {
    while (running && Date.now() - startTime < TEST_DURATION_MS) {
      if (Date.now() - lastWalletCreation > 10000) {
        const targetNode = walletsCreated % 2 === 0 ? 'node1' : 'node2';
        const wallet = await createAndFundWallet(targetNode);
        if (wallet) {
          walletsCreated++;
          lastWalletCreation = Date.now();
        }
      }
      await new Promise(r => setTimeout(r, 1000));
    }
  };
  
  const transactionLoop = async () => {
    while (running && Date.now() - startTime < TEST_DURATION_MS) {
      const wallets = Array.from(testWallets.values()).filter(w => !w.disappeared);
      if (wallets.length >= 2) {
        const from = wallets[Math.floor(Math.random() * wallets.length)];
        let to = wallets[Math.floor(Math.random() * wallets.length)];
        while (to.address === from.address && wallets.length > 1) {
          to = wallets[Math.floor(Math.random() * wallets.length)];
        }
        
        const nodeUrl = Math.random() > 0.5 ? NODE_1_URL : NODE_2_URL;
        const nonce = Math.max(from.nonceNode1 || 0, from.nonceNode2 || 0);
        
        const result = await sendTransaction(
          nodeUrl,
          from.address,
          to.address,
          0.01,
          nonce,
          from.privateKey
        );
        
        if (!result.success && result.error?.includes('nonce')) {
          logEvent({
            timestamp: Date.now(),
            type: 'nonce_mismatch',
            node: nodeUrl === NODE_1_URL ? 'node1' : 'node2',
            details: {
              address: from.address,
              attemptedNonce: nonce,
              error: result.error
            }
          });
        }
      }
      await new Promise(r => setTimeout(r, 5000));
    }
  };
  
  log('Starting test loops...');
  
  await Promise.all([
    monitorLoop(),
    walletCreationLoop(),
    transactionLoop()
  ]);
  
  running = false;
  
  console.log('\n' + '='.repeat(70));
  console.log('TEST RESULTS');
  console.log('='.repeat(70));
  
  const disappearedWallets = Array.from(testWallets.values()).filter(w => w.disappeared);
  const reappearedWallets = disappearedWallets.filter(w => w.reappeared);
  const permanentlyLost = disappearedWallets.filter(w => !w.reappeared);
  
  console.log(`\nWallets created: ${testWallets.size}`);
  console.log(`Wallets that disappeared: ${disappearedWallets.length}`);
  console.log(`Wallets that reappeared: ${reappearedWallets.length}`);
  console.log(`Wallets PERMANENTLY LOST: ${permanentlyLost.length}`);
  
  if (permanentlyLost.length > 0) {
    console.log('\nPermanently Lost Wallets:');
    for (const w of permanentlyLost) {
      console.log(`  - ${w.address} (created on ${w.createdOn} at ${new Date(w.createdAt).toISOString()})`);
      console.log(`    Last seen Node1: ${w.lastSeenNode1 ? new Date(w.lastSeenNode1).toISOString() : 'never'}`);
      console.log(`    Last seen Node2: ${w.lastSeenNode2 ? new Date(w.lastSeenNode2).toISOString() : 'never'}`);
      console.log(`    Disappeared at: ${w.disappearedAt ? new Date(w.disappearedAt).toISOString() : 'unknown'}`);
    }
  }
  
  const eventsByType = new Map<string, number>();
  for (const event of events) {
    eventsByType.set(event.type, (eventsByType.get(event.type) || 0) + 1);
  }
  
  console.log('\nEvent Summary:');
  for (const [type, count] of eventsByType) {
    console.log(`  ${type}: ${count}`);
  }
  
  console.log('\nCheckpoint Mismatches:');
  const cpMismatches = events.filter(e => e.type === 'checkpoint_mismatch');
  if (cpMismatches.length === 0) {
    console.log('  None detected');
  } else {
    for (const e of cpMismatches.slice(-5)) {
      console.log(`  Height ${e.details.height}: ${e.details.node1Root} vs ${e.details.node2Root}`);
    }
  }
  
  console.log('\nNonce Mismatches (last 10):');
  const nonceMismatches = events.filter(e => e.type === 'nonce_mismatch');
  if (nonceMismatches.length === 0) {
    console.log('  None detected');
  } else {
    for (const e of nonceMismatches.slice(-10)) {
      console.log(`  ${e.details.address}: node1=${e.details.node1Nonce || 'N/A'}, node2=${e.details.node2Nonce || 'N/A'}`);
    }
  }
  
  const [finalAccounts1, finalAccounts2] = await Promise.all([
    getAccounts(NODE_1_URL),
    getAccounts(NODE_2_URL)
  ]);
  
  console.log('\nFinal State:');
  console.log(`  Node 1 accounts: ${finalAccounts1.length}`);
  console.log(`  Node 2 accounts: ${finalAccounts2.length}`);
  
  const set1 = new Set(finalAccounts1.map(a => a.fingerprint));
  const set2 = new Set(finalAccounts2.map(a => a.fingerprint));
  
  const onlyOnNode1 = finalAccounts1.filter(a => !set2.has(a.fingerprint));
  const onlyOnNode2 = finalAccounts2.filter(a => !set1.has(a.fingerprint));
  
  if (onlyOnNode1.length > 0) {
    console.log(`\n  Accounts ONLY on Node 1 (${onlyOnNode1.length}):`);
    for (const a of onlyOnNode1.slice(0, 10)) {
      console.log(`    - ${a.fingerprint}: ${a.balance} RKU, nonce ${a.nonce}`);
    }
  }
  
  if (onlyOnNode2.length > 0) {
    console.log(`\n  Accounts ONLY on Node 2 (${onlyOnNode2.length}):`);
    for (const a of onlyOnNode2.slice(0, 10)) {
      console.log(`    - ${a.fingerprint}: ${a.balance} RKU, nonce ${a.nonce}`);
    }
  }
  
  console.log('\n' + '='.repeat(70));
  if (permanentlyLost.length === 0 && onlyOnNode1.length === 0 && onlyOnNode2.length === 0) {
    console.log('SUCCESS: No data loss detected');
  } else {
    console.log('FAILURE: Data inconsistencies detected');
    console.log(`  - ${permanentlyLost.length} wallets permanently lost`);
    console.log(`  - ${onlyOnNode1.length} accounts only on Node 1`);
    console.log(`  - ${onlyOnNode2.length} accounts only on Node 2`);
  }
  console.log('='.repeat(70));
}

process.on('SIGINT', () => {
  log('Stopping test...');
  running = false;
});

runTest().catch(console.error);
